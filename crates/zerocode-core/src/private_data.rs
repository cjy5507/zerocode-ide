//! What a kept copy must not carry — a home directory, a private address, a
//! mailbox, a credential — and the mask that takes each out of text (t-32798).
//!
//! The shapes are `tools/release/pii-rules.json`, the table the release gate
//! (`pii-scan.py`) reads as well: what the gate refuses in the published tree is
//! what this masks in the copy the ledger keeps of a worker's hand-in, and the
//! two cannot disagree because there is one list. A value the table forgives
//! (`/Users/dev`, `someone@example.com`, a fixture token that says so in its own
//! body) is left as it was written.
//!
//! What stands in a value's place is the placeholder vocabulary `AGENTS.md`
//! already writes for these things, so a masked copy re-scans clean by
//! construction ([`is_clean`]) and a reader still sees where a value stood.
//! Credentials are masked twice over: by the table's provider shapes, then by
//! what NAMES one (`KEY=value`, `Authorization:`, `Bearer`) through the core's
//! one credential table ([`crate::credential`]) — and never by how a word
//! looks, which reads `disk-guard`, `task-list` and every long path as a key.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The table, embedded: the gate's file is the build's input, so a row cannot
/// be added to one reader and forgotten by the other.
const TABLE: &str = include_str!("../../../tools/release/pii-rules.json");

/// The account name a masked home directory keeps: the first name the gate's
/// own `allow` row forgives, and the one `AGENTS.md` says to write.
pub const HOME_NAME: &str = "dev";
/// A masked private address: RFC 5737 TEST-NET-1, which no network routes and
/// the gate's `private-ip` row never reaches.
pub const PRIVATE_IP: &str = "192.0.2.1";
/// A masked mailbox: an RFC 2606 example domain, which the gate's `email` row
/// forgives.
pub const MAILBOX: &str = "someone@example.com";

/// A kind of value a kept copy must not carry — one row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Credential,
    HomePath,
    PrivateIp,
    Email,
}

impl Category {
    /// The order the mask works in. A credential goes first: a token that
    /// carries an `@` or a path is one value, and the rows after it must not
    /// find half of it.
    pub const ALL: [Self; 4] = [
        Self::Credential,
        Self::HomePath,
        Self::PrivateIp,
        Self::Email,
    ];

    /// The row's name in the table.
    const fn row(self) -> &'static str {
        match self {
            Self::Credential => "credential",
            Self::HomePath => "home-path",
            Self::PrivateIp => "private-ip",
            Self::Email => "email",
        }
    }
}

/// How many values of each kind a mask took out — what a row says about a kept
/// file, so a person reads that something was changed and what kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Found {
    pub credential: u32,
    pub home_path: u32,
    pub private_ip: u32,
    pub email: u32,
}

impl Found {
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.credential
            .saturating_add(self.home_path)
            .saturating_add(self.private_ip)
            .saturating_add(self.email)
    }

    pub fn add(&mut self, other: Self) {
        self.credential = self.credential.saturating_add(other.credential);
        self.home_path = self.home_path.saturating_add(other.home_path);
        self.private_ip = self.private_ip.saturating_add(other.private_ip);
        self.email = self.email.saturating_add(other.email);
    }

    fn bump(&mut self, category: Category, by: u32) {
        let slot = match category {
            Category::Credential => &mut self.credential,
            Category::HomePath => &mut self.home_path,
            Category::PrivateIp => &mut self.private_ip,
            Category::Email => &mut self.email,
        };
        *slot = slot.saturating_add(by);
    }
}

/// The text with its private values taken out, and what was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    pub found: Found,
}

/// One row, compiled: `find` is what the category looks like, `allow` (anchored
/// to the whole found value, as the gate's `fullmatch` is) forgives it.
struct Rule {
    category: Category,
    find: Regex,
    allow: Regex,
}

/// The table's rows, compiled once. The table is a build input pinned by this
/// module's tests, so a row that does not compile is a bug found there, never
/// in a window.
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Row {
        name: String,
        find: String,
        allow: String,
    }
    #[derive(Deserialize)]
    struct Table {
        rules: Vec<Row>,
    }
    let table: Table =
        serde_json::from_str(TABLE).expect("the shared private-value table is valid JSON");
    Category::ALL
        .iter()
        .map(|&category| {
            let row = table
                .rules
                .iter()
                .find(|row| row.name == category.row())
                .expect("the shared table has a row for every category");
            Rule {
                category,
                find: Regex::new(&row.find).expect("a row's `find` is a valid regex"),
                allow: Regex::new(&format!("^(?:{})$", row.allow))
                    .expect("a row's `allow` is a valid regex"),
            }
        })
        .collect()
});

/// What replaces one found value.
fn replacement(category: Category, value: &str) -> String {
    match category {
        Category::Credential => crate::credential::MASK.to_string(),
        // Everything but the account name stays: the path below the home
        // directory is the evidence, the person's name is the secret.
        Category::HomePath => {
            let name_from = value
                .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'))
                .map_or(0, |at| at + 1);
            format!("{}{HOME_NAME}", &value[..name_from])
        }
        Category::PrivateIp => PRIVATE_IP.to_string(),
        Category::Email => MAILBOX.to_string(),
    }
}

/// One row over `text`: the replaced text when a value was found and not
/// forgiven, `None` when the text stands as it is.
fn sweep(text: &str, rule: &Rule, found: &mut Found) -> Option<String> {
    let mut hits = 0u32;
    let swept = rule.find.replace_all(text, |caps: &regex::Captures<'_>| {
        let value = &caps[0];
        if rule.allow.is_match(value) {
            value.to_string()
        } else {
            hits += 1;
            replacement(rule.category, value)
        }
    });
    match swept {
        Cow::Owned(owned) if hits > 0 => {
            found.bump(rule.category, hits);
            Some(owned)
        }
        _ => None,
    }
}

/// The credential values something NAMES — `KEY=value`, `Authorization:`,
/// `Bearer`, `--password x`, a URL's userinfo — by the core's credential table.
fn sweep_named(text: &str, found: &mut Found) -> Option<String> {
    let masked = crate::credential::mask_named_values(text);
    if masked == text {
        return None;
    }
    // Counted by the marks the pass left, and at least one: a URL's userinfo is
    // scrubbed to a mark of its own (`clone::scrub_credentials`), and a masked
    // quoted value can swallow a mark that was already there.
    let mark = crate::credential::MASK;
    let taken = masked
        .matches(mark)
        .count()
        .saturating_sub(text.matches(mark).count())
        .max(1);
    found.bump(
        Category::Credential,
        u32::try_from(taken).unwrap_or(u32::MAX),
    );
    Some(masked)
}

/// `text` with every private value taken out and counted. The text comes back
/// whole — lines, spacing and every word the table does not name — so a report
/// stays a report.
#[must_use]
pub fn mask(text: &str) -> Masked {
    let mut found = Found::default();
    let mut owned: Option<String> = None;
    for rule in RULES.iter() {
        let current = owned.as_deref().unwrap_or(text);
        if let Some(next) = sweep(current, rule, &mut found) {
            owned = Some(next);
        }
        if rule.category == Category::Credential {
            let current = owned.as_deref().unwrap_or(text);
            if let Some(next) = sweep_named(current, &mut found) {
                owned = Some(next);
            }
        }
    }
    Masked {
        text: owned.unwrap_or_else(|| text.to_string()),
        found,
    }
}

/// Whether the table finds nothing in `text` that it does not forgive — the
/// check a masked copy passes before anything is written, so a mask that missed
/// a value fails closed instead of keeping it.
#[must_use]
pub fn is_clean(text: &str) -> bool {
    RULES.iter().all(|rule| {
        rule.find
            .find_iter(text)
            .all(|hit| rule.allow.is_match(hit.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fictional values that belong to nobody; each carries its own waiver so
    // this file does not trip the gate whose table it tests.
    // pii-scan: allow home-path — a fictional account the mask is tested with
    const HOME: &str = "/Users/mallory";
    // pii-scan: allow private-ip — a fictional subnet the mask is tested with
    const OFFICE_IP: &str = "10.99.44.7";
    // pii-scan: allow email — a fictional mailbox the mask is tested with
    const MAILBOX_OF_A_PERSON: &str = "chief@northwind-holdings.co.kr";
    // pii-scan: allow credential — a fictional key id the mask is tested with
    const KEY_ID: &str = "AKIAQ7RVBNMLKJHGFDSZ";

    #[test]
    fn the_table_has_a_row_for_every_category_and_no_row_nobody_reads() {
        let table: serde_json::Value = serde_json::from_str(TABLE).expect("valid JSON");
        let names: Vec<&str> = table["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .map(|row| row["name"].as_str().expect("a name"))
            .collect();
        for category in Category::ALL {
            assert!(names.contains(&category.row()), "{category:?} has no row");
        }
        assert_eq!(
            names.len(),
            Category::ALL.len(),
            "a row the keeper does not read would be masked by the gate alone: {names:?}"
        );
        assert_eq!(RULES.len(), Category::ALL.len());
    }

    #[test]
    fn each_kind_of_private_value_is_taken_out_and_counted() {
        let text = format!(
            "worktree {HOME}/work/t-1 on {OFFICE_IP} mailed {MAILBOX_OF_A_PERSON} with {KEY_ID}\n"
        );
        let masked = mask(&text);
        for secret in ["mallory", OFFICE_IP, MAILBOX_OF_A_PERSON, KEY_ID] {
            assert!(
                !masked.text.contains(secret),
                "{secret} survived: {}",
                masked.text
            );
        }
        assert_eq!(
            masked.text,
            format!(
                "worktree /Users/{HOME_NAME}/work/t-1 on {PRIVATE_IP} mailed {MAILBOX} with {}\n",
                crate::credential::MASK
            )
        );
        assert_eq!(
            masked.found,
            Found {
                credential: 1,
                home_path: 1,
                private_ip: 1,
                email: 1
            }
        );
        assert!(is_clean(&masked.text));
    }

    #[test]
    fn a_home_directory_keeps_its_separator_and_its_path() {
        let slug = format!("session Users-{}", HOME.rsplit('/').next().expect("a name"));
        let windows = format!("C:\\Users\\{}\\x", HOME.rsplit('/').next().expect("a name"));
        assert_eq!(mask(&slug).text, format!("session Users-{HOME_NAME}"));
        assert_eq!(mask(&windows).text, format!("C:\\Users\\{HOME_NAME}\\x"));
    }

    #[test]
    fn what_the_table_forgives_is_left_as_written() {
        let text = "/Users/dev/x 10.0.0.5 one@example.com AKIAIOSFODNN7EXAMPLE\n";
        let masked = mask(text);
        assert_eq!(masked.text, text);
        assert_eq!(masked.found, Found::default());
    }

    #[test]
    fn prose_and_identifiers_are_not_read_as_keys() {
        // The words the credential module's by-shape reading would mask:
        // `sk-` sits inside `disk-guard`, `task-list` and `ask-wait`, and a
        // long path with a digit looks like an opaque token.
        let text = "the disk-guard job DISK_FLOOR task-list ask-wait \
                    1791083855-w-35393-t32796-red-run \
                    crates/zerocode-core/src/orchestration/tests.rs:6430 tokens: 5000\n";
        let masked = mask(text);
        assert_eq!(masked.text, text);
        assert_eq!(masked.found, Found::default());
    }

    #[test]
    fn a_value_that_names_itself_is_masked_and_counted_as_a_credential() {
        for (said, gone) in [
            ("export DB_PASSWORD=hunter2 && make", "hunter2"),
            ("curl -H 'Authorization: Bearer abc.def' x", "abc.def"),
            ("gh --token ghx12345 pr list", "ghx12345"),
            ("git clone https://user:tok3n@host.test/r.git now", "tok3n"),
        ] {
            let masked = mask(said);
            assert!(
                !masked.text.contains(gone),
                "{said:?} kept {gone:?}: {}",
                masked.text
            );
            assert_eq!(masked.found.credential, 1, "{said:?}: {:?}", masked.found);
            assert_eq!(masked.found.total(), 1, "{said:?}: {:?}", masked.found);
        }
        assert!(
            mask("export DB_PASSWORD=hunter2")
                .text
                .contains("DB_PASSWORD="),
            "the key stays, so a reader sees what was taken out"
        );
    }

    #[test]
    fn a_masked_text_masks_to_itself() {
        let text = format!("{HOME}/a {OFFICE_IP} {MAILBOX_OF_A_PERSON} {KEY_ID} API_KEY=zzz\n");
        let once = mask(&text);
        let twice = mask(&once.text);
        assert_eq!(twice.text, once.text);
        assert_eq!(twice.found, Found::default());
    }

    #[test]
    fn lines_and_spacing_come_back_whole() {
        let text = format!("  one  {HOME}\r\n\ttwo\n\n three  \n");
        let masked = mask(&text);
        assert_eq!(
            masked.text,
            format!("  one  /Users/{HOME_NAME}\r\n\ttwo\n\n three  \n")
        );
    }

    #[test]
    fn a_text_with_nothing_to_take_costs_one_copy_and_changes_nothing() {
        let text = "just a report with no value in it\n".repeat(100);
        let masked = mask(&text);
        assert_eq!(masked.text, text);
        assert_eq!(masked.found.total(), 0);
        assert!(is_clean(&text));
    }

    #[test]
    fn found_counts_add_and_round_trip() {
        let mut one = Found {
            home_path: 2,
            ..Found::default()
        };
        one.add(Found {
            email: 1,
            home_path: 1,
            ..Found::default()
        });
        assert_eq!((one.home_path, one.email, one.total()), (3, 1, 4));
        let said = serde_json::to_string(&one).expect("serializes");
        assert_eq!(serde_json::from_str::<Found>(&said).expect("reads"), one);
        // A reader that meets a field it does not know, or lacks one it
        // expects, still reads the rest.
        assert_eq!(
            serde_json::from_str::<Found>(r#"{"email":2,"later":1}"#).expect("reads"),
            Found {
                email: 2,
                ..Found::default()
            }
        );
    }
}
