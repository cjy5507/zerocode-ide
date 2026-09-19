//! The guarded money path (docs/design/flow-engine-guarded-money-path.md):
//! what stands between a Flow's money step and the hand. Pure but for one
//! file — the Flow's ledger — and the caller's own step road: the
//! transaction is the caller's parameters (never the page's), the page is a
//! witness asked at no wait, one ledger line is on disk before the hand
//! moves, and a confirmation is bound to the transaction it confirms. The
//! words (the codes, the amount grammar, the confirm rule) are the core's;
//! this file only puts them in order and keeps the ledger.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::computer_flow::{Amount, Check, CheckKind, Confirm, Money, Observed};
use zerocode_core::computer_recipe::{
    RECIPE_PARAM_CLOSE, RECIPE_PARAM_OPEN, RecipeLine, RecipeTool,
};
use zerocode_core::computer_use_protocol::error_code;

/// The ledger's extension, beside its recipe: `<slug>.ledger.jsonl`.
pub const LEDGER_EXTENSION: &str = "ledger.jsonl";
/// The flag a desktop money step names its app by, and the witness aims at.
const APP_FLAG: &str = "--app";
/// The desktop witness: the check verb the Flow already judges, asked with
/// no wait by the walk (`FLOW_BASELINE_PROBE_MS`); and the browser's.
const DESKTOP_WITNESS: [&str; 1] = ["wait-for"];
const DESKTOP_WITNESS_TEXT_FLAG: &str = "--text";
const BROWSER_WITNESS: [&str; 1] = ["find"];

/// Where a transaction stands in the ledger (design §3, §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The person (or the cap) let the hand through: written right before
    /// `acting`, so the ledger says who confirmed even if the next line
    /// never came.
    Confirmed,
    /// Written before the hand moves: from here the transaction is spent.
    Acting,
    /// The line watching the money step passed: this screen said done.
    Acted,
    /// It did not: the person looks. The transaction may be tried again.
    Failed,
}

impl State {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Acting => "acting",
            Self::Acted => "acted",
            Self::Failed => "failed",
        }
    }

    /// Whether a transaction in this state is spent: the hand moved, or is
    /// moving, and does not move again.
    #[must_use]
    pub const fn spent(self) -> bool {
        matches!(self, Self::Acting | Self::Acted)
    }
}

/// Who confirmed the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    Person,
    Auto,
}

/// One ledger line, as the design writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub txn: String,
    pub amount: String,
    pub recipient: String,
    pub at_epoch_ms: i64,
    pub state: State,
    /// The evidence folder of the run that wrote the line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// The money step, counted from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    pub by: By,
}

/// The transaction a run was given: the values of the money line's three
/// parameters, as the caller wrote them (the amount validated, kept as
/// written so the page is asked for the same digits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub txn: String,
    pub amount: String,
    pub recipient: String,
}

/// Why a ledger line was not written.
#[derive(Debug)]
pub enum Refusal {
    /// An acting line for a transaction already spent (§3): not twice.
    Seen(State),
    Io(io::Error),
}

impl PartialEq for Refusal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Seen(mine), Self::Seen(theirs)) => mine == theirs,
            (Self::Io(mine), Self::Io(theirs)) => mine.kind() == theirs.kind(),
            _ => false,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seen(state) => write!(out, "already {}", state.as_str()),
            Self::Io(error) => write!(out, "{error}"),
        }
    }
}

/// A Flow's ledger (§3): one append-only file beside the recipe, one line
/// per state change, read and written under the file's own lock — so two
/// panes that reach the same money step cannot both write `acting` for one
/// transaction: the second reads the first's line and is refused.
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// The ledger of the recipe at `recipe`: `<slug>.ledger.jsonl` beside it.
    #[must_use]
    pub fn beside(recipe: &Path) -> Self {
        Self {
            path: recipe.with_extension(LEDGER_EXTENSION),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every line, in order. A line that does not read is a broken ledger,
    /// never a line skipped: a money ledger fails closed.
    fn lines(file: &mut File) -> io::Result<Vec<Line>> {
        let mut held = String::new();
        file.read_to_string(&mut held)?;
        held.lines()
            .map(|row| {
                serde_json::from_str(row).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("a ledger line does not read ({error}): {row}"),
                    )
                })
            })
            .collect()
    }

    /// The transaction's latest state among `lines`.
    fn latest(lines: &[Line], txn: &str) -> Option<State> {
        lines
            .iter()
            .rev()
            .find(|line| line.txn == txn)
            .map(|line| line.state)
    }

    /// Where the transaction stands, or None when the ledger never saw it.
    pub fn seen(&self, txn: &str) -> io::Result<Option<State>> {
        match File::open(&self.path) {
            Ok(mut file) => {
                file.lock_shared()?;
                Ok(Self::latest(&Self::lines(&mut file)?, txn))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Append `lines` together, durably — one lock, one write, one full
    /// sync (on Apple disks `sync_data` is `F_FULLFSYNC`, ~10 ms measured:
    /// the price of a hand that moves only on a line that is on the disk,
    /// paid once per call). An `acting` line among them is refused when
    /// the ledger already holds its transaction spent — judged under the
    /// exclusive lock, so no two writers can both be first — and then
    /// nothing of the call is written.
    pub fn record(&self, lines: &[Line]) -> Result<(), Refusal> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(Refusal::Io)?;
        file.lock().map_err(Refusal::Io)?;
        if let Some(acting) = lines.iter().find(|line| line.state == State::Acting)
            && let Some(state) =
                Self::latest(&Self::lines(&mut file).map_err(Refusal::Io)?, &acting.txn)
                    .filter(|state| state.spent())
        {
            return Err(Refusal::Seen(state));
        }
        let mut rows = String::new();
        for line in lines {
            let row = serde_json::to_string(line)
                .map_err(|error| Refusal::Io(io::Error::other(error)))?;
            rows.push_str(&row);
            rows.push('\n');
        }
        file.write_all(rows.as_bytes()).map_err(Refusal::Io)?;
        file.sync_data().map_err(Refusal::Io)
    }
}

/// A parameter as the person reads it: `{{name}}`.
fn placeholder(name: &str) -> String {
    format!("{RECIPE_PARAM_OPEN}{name}{RECIPE_PARAM_CLOSE}")
}

/// The transaction the money line names, from the values the run was given
/// (§1): every name with a value, the amount a plain number. Otherwise why
/// not — the message of `flow_money_unbound`.
pub fn transaction(money: &Money, values: &Map<String, Value>) -> Result<Transaction, String> {
    let mut missing = Vec::new();
    let mut given = money.names().map(|name| {
        let value = values.get(name).and_then(Value::as_str);
        if value.is_none() {
            missing.push(placeholder(name));
        }
        value.unwrap_or_default().to_string()
    });
    if !missing.is_empty() {
        return Err(format!(
            "this Flow's `{}` line needs --params for {}: the transaction is the caller's to give",
            zerocode_core::computer_flow::FLOW_KEY_MONEY,
            missing.join(", ")
        ));
    }
    Amount::read(&given[1])
        .map_err(|why| format!("{} is `{}`: {why}", placeholder(&money.amount), given[1]))?;
    let [txn, amount, recipient] = std::mem::take(&mut given);
    Ok(Transaction {
        txn,
        amount,
        recipient,
    })
}

/// Who confirms this transaction (§4): `--confirm <txn>` is the person's
/// word when it names this transaction, and a mistake
/// (`flow_confirm_unbound`) when it names another; without it, an auto cap
/// covers the amount or nobody has confirmed yet (`None`: the walk stops
/// at the money step for the person).
pub fn confirmation(
    confirm: &Confirm,
    txn: &Transaction,
    flag: Option<&str>,
) -> Result<Option<By>, &'static str> {
    match flag {
        Some(given) if given == txn.txn => Ok(Some(By::Person)),
        Some(_) => Err(error_code::FLOW_CONFIRM_UNBOUND),
        None => {
            let amount = Amount::read(&txn.amount).map_err(|_| error_code::FLOW_MONEY_UNBOUND)?;
            Ok(confirm.covers(&amount).then_some(By::Auto))
        }
    }
}

/// The value after `flag` among a step's words.
fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    argv.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

/// The two witnesses of a money step (§1): the amount and the recipient,
/// each asked for at the step's own door — the pane a browser step presses,
/// the app a desktop step names — as check lines the walk already judges,
/// paired with the text each looks for.
fn witnesses(line: &RecipeLine, txn: &Transaction) -> Result<Vec<(String, Check)>, String> {
    let aim: Vec<String> = match line.tool {
        RecipeTool::Browser => {
            let label = line
                .argv
                .get(1)
                .ok_or("the money step names no pane to witness it")?;
            BROWSER_WITNESS
                .iter()
                .map(ToString::to_string)
                .chain([label.clone()])
                .collect()
        }
        RecipeTool::Computer => {
            let app = flag_value(&line.argv, APP_FLAG).ok_or(
                "the money step names no app to witness it: a press by point alone has no witness",
            )?;
            DESKTOP_WITNESS
                .iter()
                .map(ToString::to_string)
                .chain([
                    APP_FLAG.to_string(),
                    app.to_string(),
                    DESKTOP_WITNESS_TEXT_FLAG.to_string(),
                ])
                .collect()
        }
        // Mobile presence checks do not establish amount/recipient witnesses.
        // The money path remains closed until that separate contract exists.
        RecipeTool::Emulator => {
            return Err(
                "the money step is on the emulator door, which has no witness yet".to_string(),
            );
        }
    };
    Ok([&txn.amount, &txn.recipient]
        .into_iter()
        .enumerate()
        .map(|(at, text)| {
            let mut argv = aim.clone();
            argv.push(text.clone());
            (
                text.clone(),
                Check {
                    id: at + 1,
                    kind: CheckKind::State,
                    required: true,
                    tool: line.tool,
                    argv,
                },
            )
        })
        .collect())
}

/// Whether the page agrees with the transaction (§1): both witnesses asked
/// through `probe` (the walk's own check road, at no wait) and both found.
/// Otherwise what was missing, or could not be looked for — the message of
/// `flow_page_disagrees`. A witness nobody could ask is a disagreement:
/// the hand moves on a page that showed the money, never on one that
/// could not be read.
pub fn page_agrees(
    line: &RecipeLine,
    txn: &Transaction,
    probe: impl FnOnce(&[Check]) -> BTreeMap<usize, Observed>,
) -> Result<(), String> {
    let witnesses = witnesses(line, txn)?;
    let checks: Vec<Check> = witnesses.iter().map(|(_, check)| check.clone()).collect();
    let observed = probe(&checks);
    let missing: Vec<String> = witnesses
        .iter()
        .filter_map(|(text, check)| match observed.get(&check.id) {
            Some(Observed::Seen(seen)) if seen.present => None,
            Some(Observed::Seen(_)) => Some(format!("`{text}` is not on the page")),
            Some(Observed::NotEvaluable(why)) => {
                Some(format!("`{text}` could not be looked for: {why}"))
            }
            None => Some(format!("`{text}` was not looked for")),
        })
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the page disagrees with the transaction: {}",
            missing.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use zerocode_core::computer_flow::{Check, CheckKind, Confirm, Money, Observed, Presence};
    use zerocode_core::computer_recipe::{RecipeLine, RecipeTool};
    use zerocode_core::computer_use_protocol::error_code;

    fn line(state: State, txn: &str, by: By) -> Line {
        Line {
            txn: txn.into(),
            amount: "50000".into(),
            recipient: "Kim".into(),
            at_epoch_ms: 1_000,
            state,
            evidence: Some("/evidence/run-1".into()),
            step: Some(3),
            by,
        }
    }

    #[test]
    fn the_ledger_refuses_a_transaction_it_has_seen_acting_or_acted() {
        let dir = tempfile::tempdir().expect("a folder");
        let recipe = dir.path().join("pay.md");
        let ledger = Ledger::beside(&recipe);
        assert_eq!(
            ledger.path(),
            dir.path().join(format!("pay.{LEDGER_EXTENSION}")),
            "the ledger sits beside its recipe, named for it"
        );
        assert_eq!(ledger.seen("TXN-1").expect("readable"), None);
        assert!(!ledger.path().exists(), "a look creates nothing");
        let began = std::time::Instant::now();
        ledger
            .record(&[line(State::Acting, "TXN-1", By::Person)])
            .expect("the first acting line");
        let wrote = began.elapsed();
        eprintln!(
            "ledger record (acting, fsync): {:.3} ms",
            wrote.as_secs_f64() * 1_000.0
        );
        assert_eq!(ledger.seen("TXN-1").unwrap(), Some(State::Acting));
        assert_eq!(
            ledger.record(&[line(State::Acting, "TXN-1", By::Person)]),
            Err(Refusal::Seen(State::Acting)),
            "a transaction acting is not acted on again"
        );
        ledger
            .record(&[line(State::Acted, "TXN-1", By::Person)])
            .expect("the outcome is always written");
        assert_eq!(ledger.seen("TXN-1").unwrap(), Some(State::Acted));
        assert_eq!(
            ledger.record(&[
                line(State::Confirmed, "TXN-1", By::Auto),
                line(State::Acting, "TXN-1", By::Auto)
            ]),
            Err(Refusal::Seen(State::Acted)),
            "a refused call writes none of its lines"
        );
        // Another transaction, a failed one, and a confirmed one may act.
        assert_eq!(ledger.seen("TXN-2").unwrap(), None);
        ledger
            .record(&[line(State::Acting, "TXN-2", By::Auto)])
            .unwrap();
        ledger
            .record(&[line(State::Failed, "TXN-2", By::Auto)])
            .unwrap();
        assert_eq!(ledger.seen("TXN-2").unwrap(), Some(State::Failed));
        ledger
            .record(&[line(State::Acting, "TXN-2", By::Auto)])
            .expect("a failed transaction is the person's to retry");
        let began = std::time::Instant::now();
        ledger
            .record(&[
                line(State::Confirmed, "TXN-3", By::Person),
                line(State::Acting, "TXN-3", By::Person),
            ])
            .unwrap();
        eprintln!(
            "ledger record (confirmed+acting in one call, fsync): {:.3} ms",
            began.elapsed().as_secs_f64() * 1_000.0
        );
        // Append-only: every line is kept in order, one JSON object each,
        // with the words the design names.
        let held = std::fs::read_to_string(ledger.path()).unwrap();
        let lines: Vec<Line> = held
            .lines()
            .map(|row| serde_json::from_str(row).expect(row))
            .collect();
        assert_eq!(lines.len(), 7, "{held}");
        assert_eq!(
            lines
                .iter()
                .map(|row| (row.txn.as_str(), row.state))
                .collect::<Vec<_>>(),
            [
                ("TXN-1", State::Acting),
                ("TXN-1", State::Acted),
                ("TXN-2", State::Acting),
                ("TXN-2", State::Failed),
                ("TXN-2", State::Acting),
                ("TXN-3", State::Confirmed),
                ("TXN-3", State::Acting),
            ]
        );
        let first: serde_json::Value = serde_json::from_str(held.lines().next().unwrap()).unwrap();
        assert_eq!(
            first,
            json!({
                "txn": "TXN-1", "amount": "50000", "recipient": "Kim", "at_epoch_ms": 1000,
                "state": "acting", "evidence": "/evidence/run-1", "step": 3, "by": "person"
            })
        );
        // A ledger the person cannot write refuses before the hand moves.
        let unwritable = Ledger::beside(&dir.path().join("missing").join("pay.md"));
        assert!(matches!(
            unwritable.record(&[line(State::Acting, "TXN-9", By::Person)]),
            Err(Refusal::Io(_))
        ));
    }

    fn money() -> Money {
        Money {
            id: "txn".into(),
            amount: "amount".into(),
            recipient: "recipient".into(),
            step: 3,
        }
    }

    fn values(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect()
    }

    #[test]
    fn a_persons_confirmation_binds_to_the_transaction_and_auto_holds_under_its_cap() {
        let given = values(&[("txn", "TXN-1"), ("amount", "50000"), ("recipient", "Kim")]);
        let txn = transaction(&money(), &given).expect("bound");
        assert_eq!(
            txn,
            Transaction {
                txn: "TXN-1".into(),
                amount: "50000".into(),
                recipient: "Kim".into(),
            }
        );
        // Unbound: a name without a value, or an amount that is no plain number.
        let unbound = transaction(&money(), &values(&[("txn", "TXN-1")])).unwrap_err();
        assert!(
            unbound.contains("{{amount}}") && unbound.contains("{{recipient}}"),
            "{unbound}"
        );
        assert!(
            transaction(
                &money(),
                &values(&[("txn", "TXN-1"), ("amount", "50,000"), ("recipient", "Kim")])
            )
            .is_err()
        );
        // The person: a confirmation is the transaction's id, or it is not one.
        assert_eq!(confirmation(&Confirm::Person, &txn, None), Ok(None));
        assert_eq!(
            confirmation(&Confirm::Person, &txn, Some("TXN-1")),
            Ok(Some(By::Person))
        );
        assert_eq!(
            confirmation(&Confirm::Person, &txn, Some("TXN-2")),
            Err(error_code::FLOW_CONFIRM_UNBOUND)
        );
        // Auto under its cap; over it, the person's rules.
        let capped = Confirm::Auto {
            cap: "50000".into(),
        };
        assert_eq!(confirmation(&capped, &txn, None), Ok(Some(By::Auto)));
        assert_eq!(
            confirmation(&capped, &txn, Some("TXN-1")),
            Ok(Some(By::Person)),
            "a person who confirmed is the one who confirmed"
        );
        assert_eq!(
            confirmation(&capped, &txn, Some("TXN-2")),
            Err(error_code::FLOW_CONFIRM_UNBOUND)
        );
        let over = Transaction {
            amount: "50000.01".into(),
            ..txn.clone()
        };
        assert_eq!(confirmation(&capped, &over, None), Ok(None));
        assert_eq!(
            confirmation(&capped, &over, Some("TXN-1")),
            Ok(Some(By::Person))
        );
        // The page is a witness: both the amount and the recipient are asked
        // for at the money step's own door — the browser pane it presses, or
        // the app the desktop step names — and both must be there.
        let browser = RecipeLine {
            step: 3,
            shown: 3,
            tool: RecipeTool::Browser,
            argv: vec!["click".into(), "browser-1".into(), "#send".into()],
            failed_then: false,
            money: true,
        };
        let mut asked: Vec<Vec<String>> = Vec::new();
        let answer = |found: &[&str]| {
            let found: Vec<String> = found.iter().map(|word| (*word).to_string()).collect();
            move |checks: &[Check]| -> BTreeMap<usize, Observed> {
                checks
                    .iter()
                    .map(|check| {
                        let present = check.argv.iter().any(|word| found.contains(word));
                        (
                            check.id,
                            Observed::Seen(Presence {
                                present,
                                count: Some(usize::from(present)),
                            }),
                        )
                    })
                    .collect()
            }
        };
        assert_eq!(
            page_agrees(&browser, &txn, |checks| {
                asked = checks.iter().map(|check| check.argv.clone()).collect();
                for check in checks {
                    assert_eq!(
                        (check.tool, check.kind, check.required),
                        (RecipeTool::Browser, CheckKind::State, true)
                    );
                }
                answer(&["50000", "Kim"])(checks)
            }),
            Ok(())
        );
        assert_eq!(
            asked,
            [
                vec!["find", "browser-1", "50000"],
                vec!["find", "browser-1", "Kim"]
            ]
        );
        let disagrees = page_agrees(&browser, &txn, answer(&["50000"])).unwrap_err();
        assert!(
            disagrees.contains("Kim"),
            "what is missing is named: {disagrees}"
        );
        let unread = page_agrees(&browser, &txn, |checks| {
            checks
                .iter()
                .map(|check| (check.id, Observed::NotEvaluable("the pane is gone".into())))
                .collect()
        })
        .unwrap_err();
        assert!(unread.contains("the pane is gone"), "{unread}");
        let desktop = RecipeLine {
            tool: RecipeTool::Computer,
            argv: vec![
                "click".into(),
                "--app".into(),
                "Bank".into(),
                "--text".into(),
                "Send".into(),
            ],
            ..browser.clone()
        };
        let mut asked: Vec<Vec<String>> = Vec::new();
        assert_eq!(
            page_agrees(&desktop, &txn, |checks| {
                asked = checks.iter().map(|check| check.argv.clone()).collect();
                answer(&["50000", "Kim"])(checks)
            }),
            Ok(())
        );
        assert_eq!(
            asked,
            [
                vec!["wait-for", "--app", "Bank", "--text", "50000"],
                vec!["wait-for", "--app", "Bank", "--text", "Kim"]
            ],
            "a desktop witness is the check verb the Flow already judges, aimed at the step's app"
        );
        let unaimed = RecipeLine {
            argv: vec![
                "mouse-click".into(),
                "--x".into(),
                "1".into(),
                "--y".into(),
                "1".into(),
            ],
            ..desktop
        };
        assert!(
            page_agrees(&unaimed, &txn, answer(&["50000", "Kim"])).is_err(),
            "a money step that names no app has no witness: fail closed"
        );
    }
}
