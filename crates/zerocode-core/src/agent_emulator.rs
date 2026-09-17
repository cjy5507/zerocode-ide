//! The agents' door into the workspace mobile emulator — the phone twin of
//! `agent_browser`, on the same chassis and read by the same table functions.
//!
//! `zerocode-emulator` (its shim in `computer_use::emulator_shim_script`) taps,
//! swipes, types, presses a hardware button, rotates and photographs the
//! device the window's emulator pane paints. A recipe line that starts with
//! the shim's word walks through this door: the walk counts its words, budgets
//! its hold, and binds a Flow's act to the app the device answered from — all
//! from the one `EMULATOR_VERBS` table, read by `agent_browser`'s row readers
//! so no counter, budget or check-teller is written twice.

use crate::agent_browser::{
    BrowserVerb, act_verb, acts_in, arity_ok_in, holds_ms_in, is_check_in, verb, verb_in,
};

/// The shim's name on every pane's PATH — the word a recipe line starts with
/// when its step goes through the emulator door (`computer_use::
/// emulator_shim_script` writes the shim under it).
pub const EMULATOR_CLI: &str = "zerocode-emulator";

/// A swipe's duration bounds (`parse_emulator_command`, `--ms`): the shortest
/// gesture the backends distinguish and the longest one a walk allows. The
/// parser reads these too, so the one number lives here.
pub const EMULATOR_SWIPE_MS_MIN: u32 = 50;
pub const EMULATOR_SWIPE_MS_MAX: u32 = 3_000;

/// How long one emulator command may hold a walk: the swipe ceiling — the
/// longest any gesture takes, and a comfortable bound on the round trip to
/// the device backend for the reads and the photograph too. Every verb of
/// this door is a single device round trip held by it; none is a check.
pub const EMULATOR_HOLD_MS: u64 = EMULATOR_SWIPE_MS_MAX as u64;

/// The one table of the emulator door's verbs (`EmulatorMethod`, the words
/// `parse_emulator_command` accepts): the words a recipe line may start with,
/// how many words each takes after itself (its flags and their values, a
/// preflight count the door's parser then confirms), how long it may hold a
/// walk, and whether it acts on the device — the gestures do, the reads and
/// the photograph do not, and none answers a check a Flow judges. The type
/// and the readers are `agent_browser`'s, shared, never copied.
pub const EMULATOR_VERBS: [BrowserVerb; 9] = [
    verb("list", 0, 1, EMULATOR_HOLD_MS),
    verb("open", 2, 5, EMULATOR_HOLD_MS),
    verb("tree", 4, 5, EMULATOR_HOLD_MS),
    act_verb("tap", 8, 9, EMULATOR_HOLD_MS),
    act_verb("swipe", 12, 15, EMULATOR_HOLD_MS),
    act_verb("text", 6, 7, EMULATOR_HOLD_MS),
    act_verb("button", 6, 7, EMULATOR_HOLD_MS),
    act_verb("rotate", 6, 7, EMULATOR_HOLD_MS),
    verb("screenshot", 4, 7, EMULATOR_HOLD_MS),
];

/// The table's row for a verb, if the door knows it.
#[must_use]
pub fn emulator_verb(word: &str) -> Option<&'static BrowserVerb> {
    verb_in(&EMULATOR_VERBS, word)
}

/// How long one emulator step may hold a walk: its verb's row, or None for a
/// word the door does not know.
#[must_use]
pub fn holds_ms(verb: &str) -> Option<u64> {
    holds_ms_in(&EMULATOR_VERBS, verb)
}

/// Whether a verb's answer is a check a Flow may judge (none of this door's,
/// this wave — the mobile oracle is a later parity).
#[must_use]
pub fn is_check(verb: &str) -> bool {
    is_check_in(&EMULATOR_VERBS, verb)
}

/// Whether a verb acts on the device it aims at.
#[must_use]
pub fn acts(verb: &str) -> bool {
    acts_in(&EMULATOR_VERBS, verb)
}

/// Whether a line's words fit its verb's arity — refused in the preflight, so
/// a recipe line that could never dispatch is not walked into a refusal.
pub fn arity_ok(argv: &[String]) -> Result<(), String> {
    arity_ok_in(&EMULATOR_VERBS, EMULATOR_CLI, argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_browser;

    fn argv(line: &[&str]) -> Vec<String> {
        line.iter().map(|word| (*word).to_string()).collect()
    }

    /// One table says what every emulator verb is — how many words it takes,
    /// how long it may hold a walk, whether it acts on the device and whether
    /// it is a check — and it is read by the very functions the browser
    /// table's rows are read by, one implementation shared given this table,
    /// never a second copy.
    #[test]
    fn one_emulator_table_holds_and_counts_every_verb_with_the_browser_tables_functions() {
        let mut seen = std::collections::BTreeSet::new();
        for row in &EMULATOR_VERBS {
            assert!(seen.insert(row.word), "`{}` twice in the table", row.word);
            assert!(row.arity.min <= row.arity.max, "{row:?}");
            // Every gesture holds the device at most the swipe ceiling, and no
            // verb of this wave is a check the Flow judges.
            assert_eq!(row.hold_ms, EMULATOR_HOLD_MS, "{row:?}");
            assert!(!row.check, "{row:?}");
            // The door's readers are the browser's, given this table: one
            // implementation, never a second.
            assert_eq!(holds_ms(row.word), Some(row.hold_ms));
            assert_eq!(
                holds_ms(row.word),
                agent_browser::holds_ms_in(&EMULATOR_VERBS, row.word)
            );
            assert_eq!(
                is_check(row.word),
                agent_browser::is_check_in(&EMULATOR_VERBS, row.word)
            );
            assert_eq!(
                acts(row.word),
                agent_browser::acts_in(&EMULATOR_VERBS, row.word)
            );
        }
        assert_eq!(EMULATOR_VERBS.len(), 9);
        assert_eq!(holds_ms("nope"), None);
        assert_eq!(EMULATOR_CLI, "zerocode-emulator");
        assert_eq!(EMULATOR_HOLD_MS, u64::from(EMULATOR_SWIPE_MS_MAX));

        for act in ["tap", "swipe", "text", "button", "rotate"] {
            assert!(acts(act), "{act} acts on the device");
        }
        for look in ["list", "open", "tree", "screenshot", "nope"] {
            assert!(!acts(look), "{look} acts on nothing");
        }
        assert!(!is_check("tap") && !is_check("screenshot") && !is_check("tree"));

        // A well-formed line of every verb fits; a short one does not — the
        // same counter the browser preflight uses, so a line that could never
        // dispatch is refused before the device is touched.
        for ok in [
            &["list"][..],
            &["open", "--platform", "ios"],
            &["tree", "--platform", "ios", "--device", "phone"],
            &[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
                "--y",
                "0.5",
            ],
            &[
                "swipe",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x1",
                "0.5",
                "--y1",
                "0.8",
                "--x2",
                "0.5",
                "--y2",
                "0.2",
            ],
            &[
                "text",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--text",
                "hi",
            ],
            &[
                "button",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--name",
                "home",
            ],
            &[
                "rotate",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--rotation",
                "1",
            ],
            &["screenshot", "--platform", "ios", "--device", "phone"],
        ] {
            assert_eq!(arity_ok(&argv(ok)), Ok(()), "{ok:?}");
        }
        for refused in [
            &[][..],
            &["nope", "--platform", "ios"],
            &[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
            ],
            &["screenshot"],
        ] {
            assert!(arity_ok(&argv(refused)).is_err(), "{refused:?}");
        }
        // The refusal names the emulator door, not the browser's.
        let why = arity_ok(&argv(&["nope"])).unwrap_err();
        assert!(why.contains("zerocode-emulator"), "{why}");
    }
}
