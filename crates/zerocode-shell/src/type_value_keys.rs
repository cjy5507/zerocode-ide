//! The key typing into a page's fields asks with, in the window's settings
//! (t-9537).
//!
//! A goal walk that meets an empty field asks the value seat's chosen row
//! (`zerocode_core::type_value::chosen`) what to type, with a key a person put
//! in the window's key store for that row — and nothing let a person put one
//! there, so every such walk refused its entry with
//! [`NO_KEY`](crate::computer_use::errand::value::NO_KEY) and left the field
//! to a person.
//!
//! The Computer Use pane offers one slot per key a walk reads. Everything a
//! slot names is the seat's table's — the key's name, the rows that read it,
//! the row a walk asks now — and the keychain item it is kept in is the one
//! the walk's writer reads, spelled by the writer's own function
//! ([`key_service`]), so the pane and the walk cannot disagree about where the
//! key is. A key only a row nobody asks would read is no slot: nothing picks
//! that row, and a key saved for it would change nothing a person could see.
//!
//! The key goes in and never comes back: the answer says whether one is
//! saved. A walk makes its writer when it starts (`LiveWriter::window`) and
//! reads the key then, so a key saved here is typed with from the next walk
//! on — no restart, and nothing here holds it.

use std::path::Path;

use serde::Serialize;
use zerocode_core::type_value::{self, GeneratorRoad, ValueRow};

use crate::api_routers::{RouterKeys, RouterRefusal};
use crate::computer_use::errand::value::{LastAnswer, endpoint_of, key_service, last_answered};
use crate::typesafe_settings::{key_saved_at, remove_key_at, save_key_at};

/// What an empty key is refused in.
const EMPTY_KEY: &str = "API 키가 비어 있습니다";
/// What a name no walk reads is refused in — without the name, which is
/// whatever the caller sent.
const NOT_A_WALKS_KEY: &str = "글자 입력이 읽는 키가 아니라 바꾸지 않았습니다";

/// One row of the seat's table that reads a slot's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyRow {
    /// The row's id — the word the seat, a walk's row and a refusal use.
    pub id: &'static str,
    /// The model the row asks.
    pub model: &'static str,
}

/// One key the pane offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueKey {
    /// The key's name in the window's key store — the table's
    /// [`ValueRow::credential_key`]. A name, never a key.
    pub credential_key: &'static str,
    /// Every row of the table that reads this key, in the table's order.
    pub rows: Vec<KeyRow>,
    /// The row a walk asks now, when it reads this key.
    pub chosen: Option<&'static str>,
    /// Whether a key is saved under it — never the key.
    pub key_saved: bool,
}

/// What the pane paints: whether a key could be kept here at all, and every
/// key a walk reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueKeys {
    pub keys_kept_here: bool,
    pub keys: Vec<ValueKey>,
}

/// One login road as the pane draws it (t-10372): which road, the model its
/// row asks, whether its CLI is on this machine, and whose account it runs as
/// — the account the window's panes run as, by the label the accounts pane
/// shows, or `None` for the machine's own login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRoad {
    pub road: GeneratorRoad,
    pub model: &'static str,
    pub installed: bool,
    pub account: Option<String>,
}

/// The whole card (t-10372): the road the person chose, each login road, the
/// last question's answer, and the keys a person may keep for the key road.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorCard {
    pub road: GeneratorRoad,
    pub logins: Vec<LoginRoad>,
    pub last: Option<LastAnswer>,
    #[serde(flatten)]
    pub keys: ValueKeys,
}

/// The account a login road's CLI runs as, by the label its accounts pane
/// shows — read off the window's account stores, never off a login.
fn account_of(road: GeneratorRoad, config_root: &Path) -> Option<String> {
    match road {
        GeneratorRoad::ClaudeLogin => {
            let store = crate::accounts::read_store(config_root);
            zerocode_core::active_account(&store.accounts, &store.selection)
                .filter(|_| !store.selection.system_default)
                .map(zerocode_core::ClaudeAccount::label)
        }
        GeneratorRoad::CodexLogin => {
            let store = crate::codex_accounts::read_store(config_root);
            zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
                .map(zerocode_core::codex_account::CodexAccount::label)
        }
        GeneratorRoad::Auto | GeneratorRoad::ApiKey | GeneratorRoad::Off => None,
    }
}

/// The card: `road` as the settings hold it, each login road with its model,
/// its CLI (`installed` answers whether the catalogue found it) and its
/// account, the window's last answer, and `keys`.
#[must_use]
pub fn card(
    road: GeneratorRoad,
    config_root: &Path,
    installed: impl Fn(GeneratorRoad) -> bool,
    keys: ValueKeys,
) -> GeneratorCard {
    let logins = GeneratorRoad::Auto
        .tries()
        .iter()
        .filter_map(|login| {
            type_value::chosen_on(*login).map(|row| LoginRoad {
                road: *login,
                model: &row.model,
                installed: installed(*login),
                account: account_of(*login, config_root),
            })
        })
        .collect();
    GeneratorCard {
        road,
        logins,
        last: last_answered(),
        keys,
    }
}

/// One key a walk reads: its name, the keychain item the walk's writer reads
/// it from, and every row of the table that reads that item.
struct Slot {
    name: &'static str,
    service: String,
    rows: Vec<&'static ValueRow>,
}

/// The name of the key a person puts in the window's key store for `row`,
/// when its road is one the walk's writer takes — a key for a road never
/// taken is a key nothing asks with.
fn named_key(row: &ValueRow) -> Option<&str> {
    row.credential_key
        .as_deref()
        .filter(|_| endpoint_of(row).is_some())
}

/// The keys the pane offers: the one the walk's writer reads — the chosen
/// row's, which is the only row a walk asks — with every row that reads the
/// same item. When something lets a person choose another row, the rows it
/// offers join here and the pane draws their keys with no change of its own.
fn offered() -> Vec<Slot> {
    let Some(chosen) = type_value::chosen() else {
        return Vec::new();
    };
    let (Some(name), Some(service)) = (named_key(chosen), key_service(chosen)) else {
        return Vec::new();
    };
    let rows = type_value::rows()
        .iter()
        .filter(|row| {
            named_key(row).is_some() && key_service(row).as_deref() == Some(service.as_str())
        })
        .collect();
    vec![Slot {
        name,
        service,
        rows,
    }]
}

/// The slot named `name`, refused when no walk reads a key by that name.
fn slot_named(name: &str) -> Result<Slot, RouterRefusal> {
    offered()
        .into_iter()
        .find(|slot| slot.name == name)
        .ok_or_else(|| NOT_A_WALKS_KEY.to_string().into())
}

/// The pane's answer: whether this machine keeps keys, and every key a walk
/// reads with whether one is saved.
///
/// # Errors
/// An unreadable keychain.
pub fn read(keys: &dyn RouterKeys, keys_kept_here: bool) -> Result<ValueKeys, RouterRefusal> {
    let chosen = type_value::chosen().map(|row| row.id.as_str());
    let listed = offered()
        .into_iter()
        .map(|slot| {
            Ok(ValueKey {
                credential_key: slot.name,
                chosen: chosen.filter(|id| slot.rows.iter().any(|row| row.id == *id)),
                key_saved: key_saved_at(&slot.service, keys)?,
                rows: slot
                    .rows
                    .into_iter()
                    .map(|row| KeyRow {
                        id: &row.id,
                        model: &row.model,
                    })
                    .collect(),
            })
        })
        .collect::<Result<_, RouterRefusal>>()?;
    Ok(ValueKeys {
        keys_kept_here,
        keys: listed,
    })
}

/// Keep `key` under the key named `name`, trimmed, in the item a walk reads
/// it from.
///
/// # Errors
/// A name no walk reads, an empty key, a machine with no keychain, or a
/// refused keychain write.
pub fn save(name: &str, key: &str, keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    save_key_at(&slot_named(name)?.service, key, EMPTY_KEY, keys)
}

/// Forget the key saved under the key named `name`. Nothing saved is not an
/// error.
///
/// # Errors
/// A name no walk reads, or a refused keychain deletion.
pub fn remove(name: &str, keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    remove_key_at(&slot_named(name)?.service, keys)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use zerocode_core::type_value::{chosen, row, rows};

    use super::*;
    use crate::api_routers::{HeldKeys, Keychain, RouterRefusalKind};

    /// A key as a person pastes it, with the space a paste brings along.
    const PASTED: &str = "  sk-a-key-a-person-pasted  ";

    /// The name of the key the chosen row reads.
    fn chosen_key() -> &'static str {
        chosen()
            .and_then(|row| row.credential_key.as_deref())
            .expect("the chosen row names its key")
    }

    /// A key store that keeps nothing and remembers every item anything tried
    /// to write or delete in it.
    #[derive(Default)]
    struct Touched(RefCell<Vec<String>>);

    impl RouterKeys for Touched {
        fn read(&self, _: &str) -> Result<Option<String>, RouterRefusal> {
            Ok(None)
        }
        fn write(&self, service: &str, _: &str) -> Result<(), RouterRefusal> {
            self.0.borrow_mut().push(service.to_string());
            Ok(())
        }
        fn delete(&self, service: &str) -> Result<(), RouterRefusal> {
            self.0.borrow_mut().push(service.to_string());
            Ok(())
        }
    }

    /// The pane offers exactly the key a walk reads — the chosen row's, with
    /// the rows that read it from the same item, each a road this product
    /// takes — and says a walk reads it now. One slot: every other key the
    /// table names is read by a row nothing picks.
    #[test]
    fn the_pane_offers_the_key_a_walk_reads_and_no_other() {
        let answer = read(&HeldKeys::default(), true).expect("the keys");
        assert!(answer.keys_kept_here);
        let [only] = answer.keys.as_slice() else {
            panic!("one slot, the chosen row's: {:?}", answer.keys);
        };
        let asked = chosen().expect("a chosen row");
        assert_eq!(only.credential_key, chosen_key());
        assert_eq!(only.chosen, Some(asked.id.as_str()), "a walk reads it now");
        assert!(
            only.rows
                .iter()
                .any(|listed| listed.id == asked.id && listed.model == asked.model),
            "{:?}",
            only.rows
        );
        for listed in &only.rows {
            let reads = row(listed.id).expect("a row of the table");
            assert_eq!(
                key_service(reads),
                key_service(asked),
                "{} reads another item",
                listed.id
            );
            assert!(
                endpoint_of(reads).is_some(),
                "{} is a road this product never takes",
                listed.id
            );
        }
        assert!(!only.key_saved, "nothing saved yet");
        let named: Vec<&str> = rows()
            .iter()
            .filter_map(|row| row.credential_key.as_deref())
            .collect();
        assert!(named.contains(&chosen_key()));
    }

    /// A key is kept trimmed in the item the walk's writer reads, reported as
    /// saved and never sent back; removing it forgets it, and removing it
    /// again is no error.
    #[test]
    fn a_key_is_kept_where_a_walk_reads_it_and_never_comes_back() {
        let keys = HeldKeys::default();
        let service = key_service(chosen().expect("a chosen row")).expect("its item");
        save(chosen_key(), PASTED, &keys).expect("saved");
        assert_eq!(keys.held(&service).as_deref(), Some(PASTED.trim()));
        let answer = read(&keys, true).expect("the keys");
        assert!(answer.keys.iter().all(|key| key.key_saved), "{answer:?}");
        let wire = serde_json::to_string(&answer).expect("serializes");
        assert!(
            !wire.contains(PASTED.trim()),
            "the pane is sent the key: {wire}"
        );

        remove(chosen_key(), &keys).expect("removed");
        assert_eq!(keys.held(&service), None);
        let answer = read(&keys, true).expect("the keys");
        assert!(answer.keys.iter().all(|key| !key.key_saved), "{answer:?}");
        remove(chosen_key(), &keys).expect("nothing saved is not an error");
    }

    /// An empty key is refused, and so is every name a walk does not read — a
    /// name the table never held, a keychain item's own name, and a key only
    /// a row nothing picks reads — and not one of them writes or deletes
    /// anything in the key store.
    #[test]
    fn an_empty_key_or_a_name_no_walk_reads_touches_nothing() {
        let keys = Touched::default();
        assert!(
            save(chosen_key(), "   ", &keys).is_err(),
            "an empty key saved"
        );
        let service = key_service(chosen().expect("a chosen row")).expect("its item");
        let mut names = vec!["", "SOME_OTHER_API_KEY", service.as_str()];
        names.extend(
            rows()
                .iter()
                .filter_map(|row| row.credential_key.as_deref())
                .filter(|name| *name != chosen_key()),
        );
        for name in names {
            assert!(save(name, "a-key", &keys).is_err(), "`{name}` saved");
            assert!(remove(name, &keys).is_err(), "`{name}` removed");
        }
        let touched = keys.0.borrow();
        assert!(touched.is_empty(), "the key store was touched: {touched:?}");
    }

    /// Off macOS there is no keychain: the pane says so before anyone types,
    /// reads nothing as saved, and a save is refused in the kind the pane
    /// translates rather than "kept" nowhere.
    #[test]
    fn a_machine_with_no_keychain_says_so_and_refuses_the_key_by_kind() {
        let store = Keychain::without_a_store();
        let answer = read(&store, store.keeps_keys()).expect("the keys");
        assert!(!answer.keys_kept_here);
        assert!(answer.keys.iter().all(|key| !key.key_saved));
        let refusal = save(chosen_key(), "a-key", &store).expect_err("refused");
        assert_eq!(refusal.kind, RouterRefusalKind::KeychainUnavailable);
    }
}
