//! What a kept copy must not carry — a home directory, a private address, a
//! mailbox, a credential — and the mask that takes each out of text (t-32798).
//!
//! Today nothing keeps a copy of what a worker hands in, so nothing masks one:
//! this is the shape the keeping asks of the mask, and a mask that takes nothing
//! out. The tests below fail on it.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// There is no shared table yet: the release gate carries its own, in the script.
const TABLE: &str = r#"{"rules":[]}"#;

pub const HOME_NAME: &str = "dev";
pub const PRIVATE_IP: &str = "192.0.2.1";
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
}

/// The text with its private values taken out, and what was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    pub found: Found,
}

/// No row is compiled: nothing reads the table.
static RULES: LazyLock<Vec<Category>> = LazyLock::new(Vec::new);

/// `text` as it is: no value is taken out.
#[must_use]
pub fn mask(text: &str) -> Masked {
    Masked {
        text: text.to_string(),
        found: Found::default(),
    }
}

/// Whether `text` holds nothing private: not asked of any text today.
#[must_use]
pub fn is_clean(_text: &str) -> bool {
    true
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
