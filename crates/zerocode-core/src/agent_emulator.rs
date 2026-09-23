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
    BrowserVerb, act_verb, acts_in, arity_ok_in, check_verb, holds_ms_in, is_check_in, verb,
    verb_in,
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
/// the device backend for reads and photographs too. Mark clicks re-read
/// the tree before input; deterministic checks keep this same policy.
pub const EMULATOR_HOLD_MS: u64 = EMULATOR_SWIPE_MS_MAX as u64;

/// The longest a press by number waits, after its tap, for the tree it led
/// to to stop changing before it answers (t-6385). A phone's look right after
/// a press reads the screen it is leaving — a walk that looked at once was
/// refused its next press 5 times of 5 (t-6350) — and the tree stops
/// changing a second before the paint does: p50 1,080 ms, at most 1,201,
/// against 2,258 for the pixels, on an iPhone 17 simulator under load 10–36.
/// Past this the press answers with the tree as it last read it, and says it
/// did not settle.
pub const EMULATOR_SETTLE_CEILING_MS: u64 = 2_000;

/// How long a press waits for its tree to start changing at all before it
/// says the press moved nothing: about twice the first paint after a tap
/// (273–301 ms, t-6350) — a press on an inert spot, or one whose screen
/// answers later than a person would call the same moment.
pub const EMULATOR_SETTLE_QUIET_MS: u64 = 600;

/// How long a mark click may hold a walk: the door's own ceiling, then its
/// screen settling.
pub const EMULATOR_CLICK_HOLD_MS: u64 = EMULATOR_HOLD_MS + EMULATOR_SETTLE_CEILING_MS;

/// The key a mark click's answer says how its screen settled under — how
/// long, in how many reads, and how it ended (`still`, `unmoved`,
/// `ceiling`) — when its door waits for that; a walk's row carries it too.
pub const EMULATOR_SETTLE_KEY: &str = "settle";

/// The one table of the emulator door's verbs (`EmulatorMethod`, the words
/// `parse_emulator_command` accepts): the words a recipe line may start with,
/// how many words each takes after itself (its flags and their values, a
/// preflight count the door's parser then confirms), how long it may hold a
/// walk, and whether it acts on the device — the gestures do, the reads and
/// the photograph do not. The find and foreground reads answer checks a Flow judges. The type
/// and the readers are `agent_browser`'s, shared, never copied.
pub const EMULATOR_VERBS: [BrowserVerb; 13] = [
    verb("list", 0, 1, EMULATOR_HOLD_MS),
    verb("open", 2, 5, EMULATOR_HOLD_MS),
    verb("tree", 4, 5, EMULATOR_HOLD_MS),
    verb("marks", 4, 7, EMULATOR_HOLD_MS),
    check_verb("find", 6, 7, EMULATOR_HOLD_MS),
    check_verb("foreground", 6, 7, EMULATOR_HOLD_MS),
    act_verb("click", 8, 12, EMULATOR_CLICK_HOLD_MS),
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

/// Whether a verb's deterministic observation is a check a Flow may judge.
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

    #[test]
    fn emulator_checks_are_reads_with_the_shared_arity_and_hold_policy() {
        for word in ["find", "foreground"] {
            let row = emulator_verb(word).expect("mobile check registered");
            assert!(is_check(word));
            assert!(!acts(word));
            assert_eq!((row.arity.min, row.arity.max), (6, 7));
            assert_eq!(holds_ms(word), Some(EMULATOR_HOLD_MS));
        }
    }

    #[test]
    fn emulator_marks_and_click_share_the_verb_table_contract() {
        // `marks … --text <words>` counts a check's words in the same look,
        // and a click holds its screen's settling, may count them in the tree
        // it settled on and may answer what that screen would carry
        // (`--preview`, t-6385).
        for (word, min, max, acts, hold) in [
            ("marks", 4, 7, false, EMULATOR_HOLD_MS),
            ("click", 8, 12, true, EMULATOR_CLICK_HOLD_MS),
        ] {
            let row = emulator_verb(word).expect("the mobile marks door is registered");
            assert_eq!((row.arity.min, row.arity.max), (min, max));
            assert_eq!(holds_ms(word), Some(hold));
            assert_eq!(super::acts(word), acts);
            assert!(!is_check(word));
            for count in [min, max] {
                let mut line = vec![word.to_string()];
                line.extend(std::iter::repeat_n("argument".to_string(), count));
                assert!(arity_ok(&line).is_ok());
            }
            for count in [min - 1, max + 1] {
                let mut line = vec![word.to_string()];
                line.extend(std::iter::repeat_n("argument".to_string(), count));
                assert!(arity_ok(&line).is_err());
            }
        }
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
            // Reads and gestures share the device hold ceiling; a mark click
            // holds its screen's settling on top of it.
            let hold = if row.word == "click" {
                EMULATOR_CLICK_HOLD_MS
            } else {
                EMULATOR_HOLD_MS
            };
            assert_eq!(row.hold_ms, hold, "{row:?}");
            assert!(!row.check || !row.acts, "checks never act: {row:?}");
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
        assert_eq!(EMULATOR_VERBS.len(), 13);
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
