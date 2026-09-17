//! Turning a task title into a name git and both filesystems will accept.
//!
//! The input is whatever the user pasted — a Korean sentence, a shell command,
//! an emoji-prefixed headline. The output is used twice, as a **path component**
//! and as a **git ref component**, so it has to satisfy the intersection of two
//! rule sets:
//!
//! - git rejects refs containing `..`, `~`, `^`, `:`, `?`, `*`, `[`, `\`, ASCII
//!   control bytes, a `.lock` suffix, a leading `.` or `-`, or `@{`.
//! - Windows rejects `< > : " / \ | ? *`, trailing dots and spaces, and the
//!   device names (`CON`, `NUL`, `COM1`…) whatever the extension.
//!
//! Rather than enumerate what to strip — the way every one of those rules gets
//! missed one at a time — this keeps an **allow-list**: ASCII alphanumerics and
//! the `-` we insert. Every character above is then unrepresentable, so the
//! only cases left to handle are an empty result and a device name.
//!
//! Non-ASCII characters are separators rather than transliterated guesses.
//! Worker checkout paths and branches therefore have the same stable ASCII
//! spelling on every filesystem; an all-non-ASCII title uses [`FALLBACK_SLUG`].

/// Longest slug we will emit. Long enough to stay readable, short enough that
/// the path stays well inside `PATH_MAX` once a worktree root and git's own
/// administrative files are appended.
pub const MAX_SLUG_CHARS: usize = 48;

/// Longest name [`crate::Orchestrator::create`] can emit after resolving a
/// collision. The create loop tries at most 100 candidates, so the largest
/// suffix is `-100` (four characters) on top of [`MAX_SLUG_CHARS`].
pub const MAX_WORKTREE_NAME_CHARS: usize = MAX_SLUG_CHARS + 4;

/// Used when a title has nothing we can keep — an all-punctuation or
/// all-non-ASCII title, or one that reduces to a Windows device name.
pub const FALLBACK_SLUG: &str = "task";

/// Windows treats these as devices in every directory, with or without an
/// extension. Creating one fails in ways that read like a permissions bug.
///
/// Windows also recognises superscript-digit aliases such as `COM¹`, but the
/// ASCII allow-list drops those digits before this check, so those spellings
/// cannot reach the filesystem and do not belong in this table.
const RESERVED_STEMS: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Reduce a title to one lowercase `-` separated component.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut separator_pending = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if separator_pending && !slug.is_empty() {
                slug.push('-');
            }
            separator_pending = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            separator_pending = true;
        }
    }

    // Cap first, then trim: a cut that lands on a separator would otherwise
    // leave a trailing `-`, which git rejects at the end of a ref component.
    let capped: String = slug.chars().take(MAX_SLUG_CHARS).collect();
    let trimmed = capped.trim_matches('-');
    if trimmed.is_empty() || is_reserved_stem(trimmed) {
        return FALLBACK_SLUG.to_string();
    }
    trimmed.to_string()
}

/// The name to try on the nth attempt, `attempt` starting at 1.
///
/// The first attempt is the bare slug, so the common case — a task whose name
/// nothing else claims — gets a clean `wt/fix-the-drain-gate` rather than
/// `wt/fix-the-drain-gate-1`.
#[must_use]
pub fn candidate(base: &str, attempt: usize) -> String {
    if attempt <= 1 {
        base.to_string()
    } else {
        format!("{base}-{attempt}")
    }
}

/// Turn a checkout basename into the component stored below a branch prefix.
///
/// Worker checkout directories stay flat (`t-1403-fix-names`) while their
/// branches group the readable slug below the ledger id
/// (`t-1403/fix-names`). Names without that leading task id, and task ids with
/// no readable slug, keep their spelling.
#[must_use]
pub fn branch_component_from_checkout(name: &str) -> String {
    let Some(task_id) = task_id_prefix(name) else {
        return name.to_string();
    };
    let Some(slug) = name
        .strip_prefix(task_id)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return name.to_string();
    };
    format!("{task_id}/{slug}")
}

/// Recover the ledger task id from a worker worktree path or branch.
///
/// Worker names lead with `t-NNN`, followed by either the readable title or a
/// collision suffix. Checkout paths and legacy branches keep that on the final
/// component (`t-1111-fix-the-gate`); current branches put the slug below the
/// id (`wt/t-1111/fix-the-gate`). Other worktrees deliberately return `None`;
/// this parser looks only at the final component or an exact task-id parent,
/// never a task-shaped substring in the middle of a name.
#[must_use]
pub fn worker_task_id(name: &str) -> Option<&str> {
    let mut components = name.rsplit('/');
    let component = components.next()?;
    if let Some(task_id) = task_id_prefix(component) {
        return Some(task_id);
    }
    let parent = components.next()?;
    let task_id = task_id_prefix(parent)?;
    (task_id.len() == parent.len()).then_some(task_id)
}

fn task_id_prefix(component: &str) -> Option<&str> {
    let suffix = component.strip_prefix("t-")?;
    let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let boundary = &suffix[digits..];
    if !boundary.is_empty() && !boundary.starts_with('-') {
        return None;
    }
    Some(&component[..2 + digits])
}

fn is_reserved_stem(value: &str) -> bool {
    RESERVED_STEMS
        .iter()
        .any(|reserved| value.eq_ignore_ascii_case(reserved))
}

/// What git refuses in a ref component, as characters and as sequences.
///
/// Kept beside [`slugify`] rather than inside it because a **prefix** does not
/// go through the slug: it is a name somebody typed for their branches
/// (`joe`, `feature/joe`) and slugifying it would silently rename it. So the
/// prefix is CHECKED instead, and the caller decides what a refusal means —
/// which is exactly the difference Orca draws (`computeValidatedBranchName`
/// drops an unusable git-username prefix and refuses a custom one).
const REF_FORBIDDEN: [char; 8] = ['~', '^', ':', '?', '*', '[', '\\', ' '];

/// Is this usable as a branch prefix, exactly as typed?
///
/// `/` is legal and meaningful — `feature/joe` is one prefix of two components
/// — so it is not on the list above; empty components are what
/// [`clean_prefix`] removes.
#[must_use]
pub fn prefix_is_usable(prefix: &str) -> bool {
    let cleaned = clean_prefix(prefix);
    if cleaned.is_empty() {
        return false;
    }
    if cleaned
        .chars()
        .any(|ch| REF_FORBIDDEN.contains(&ch) || ch.is_control())
    {
        return false;
    }
    if cleaned.contains("..") || cleaned.contains("@{") {
        return false;
    }
    cleaned.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('-')
            && !part.starts_with('.')
            && !part.ends_with('.')
            && !part.ends_with(".lock")
    })
}

/// A prefix with its edges and its doubled separators taken off — the shape
/// `prefix + "/" + name` can be built from without producing `//` or a
/// leading `/`.
#[must_use]
pub fn clean_prefix(prefix: &str) -> String {
    prefix
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prose_title_becomes_one_readable_component() {
        assert_eq!(
            slugify("Fix the flaky drain gate test"),
            "fix-the-flaky-drain-gate-test"
        );
    }

    #[test]
    fn korean_words_are_treated_as_separators() {
        assert_eq!(slugify("드레인 drain 게이트 gate"), "drain-gate");
    }

    #[test]
    fn a_korean_title_leaves_only_its_ascii_words() {
        assert_eq!(slugify("t-1400 복원 워커는 제 탭으로"), "t-1400");
        assert_eq!(slugify("복원 워커 restored tab"), "restored-tab");
        assert_eq!(slugify("복원 워커"), FALLBACK_SLUG);
    }

    #[test]
    fn runs_of_punctuation_collapse_and_the_edges_are_trimmed() {
        assert_eq!(
            slugify("  --refactor:: the __gate!!  "),
            "refactor-the-gate"
        );
    }

    /// Each of these is a sequence git or Windows rejects outright. The
    /// allow-list has to make them unrepresentable, not merely unlikely.
    #[test]
    fn every_character_git_or_windows_forbids_is_dropped() {
        let hostile = "a..b~c^d:e?f*g[h\\i@{j\u{7}k<l>m\"n|o/p";
        let slug = slugify(hostile);
        for forbidden in [
            '.', '~', '^', ':', '?', '*', '[', '\\', '@', '{', '<', '>', '"', '|', '/', '\u{7}',
        ] {
            assert!(
                !slug.contains(forbidden),
                "{forbidden:?} survived in {slug:?}"
            );
        }
        assert_eq!(slug, "a-b-c-d-e-f-g-h-i-j-k-l-m-n-o-p");
    }

    #[test]
    fn a_slug_never_starts_or_ends_with_a_separator() {
        for title in ["-leading", "trailing-", "...both...", "!"] {
            let slug = slugify(title);
            assert!(!slug.starts_with('-'), "{slug:?}");
            assert!(!slug.ends_with('-'), "{slug:?}");
        }
    }

    #[test]
    fn a_title_with_nothing_to_keep_falls_back_rather_than_producing_an_empty_ref() {
        assert_eq!(slugify(""), FALLBACK_SLUG);
        assert_eq!(slugify("   \n\t"), FALLBACK_SLUG);
        assert_eq!(slugify("!!! ??? ***"), FALLBACK_SLUG);
    }

    /// `con/` is not a directory anyone can create on Windows, and the failure
    /// surfaces as an opaque OS error rather than as "bad name".
    #[test]
    fn windows_device_names_are_never_emitted() {
        for title in ["CON", "nul", "  Com1  ", "LPT9"] {
            assert_eq!(slugify(title), FALLBACK_SLUG, "title was {title:?}");
        }
        // Only the exact stem is reserved; a longer name that contains it is fine.
        assert_eq!(slugify("console rendering"), "console-rendering");
    }

    /// Superscript digits are non-ASCII and therefore separators. They cannot
    /// reproduce Windows' superscript device aliases in an emitted name.
    #[test]
    fn superscript_device_aliases_are_unrepresentable() {
        for (title, expected) in [("COM¹", "com"), ("com²", "com"), (" Lpt³ ", "lpt")] {
            assert_eq!(slugify(title), expected, "title was {title:?}");
        }
        assert_eq!(slugify("com⁴"), "com");
    }

    #[test]
    fn long_titles_are_capped_without_leaving_a_trailing_separator() {
        let slug = slugify(&"word ".repeat(40));
        assert_eq!(slug.chars().count(), MAX_SLUG_CHARS);
        assert!(!slug.ends_with('-'), "{slug:?}");

        // The case that regresses: the cut lands exactly on the separator, so
        // trimming has to happen after capping and not before.
        let slug = slugify(&format!("{} b", "a".repeat(MAX_SLUG_CHARS - 1)));
        assert_eq!(slug, "a".repeat(MAX_SLUG_CHARS - 1));
    }

    #[test]
    fn a_long_mixed_title_is_ascii_and_capped_after_korean_is_dropped() {
        let slug = slugify(&"가나다라마 word ".repeat(30));
        assert!(slug.chars().count() <= MAX_SLUG_CHARS);
        assert!(slug.is_ascii(), "non-ASCII survived in {slug:?}");
        assert!(!slug.ends_with('-'), "{slug:?}");
    }

    /// A prefix is somebody's own word for their branches, so it is checked
    /// rather than slugified — and every sequence git refuses in a ref has to
    /// be caught here, because the alternative is `git worktree add` failing
    /// with a message about a name the person never typed.
    #[test]
    fn a_prefix_git_would_refuse_is_named_as_unusable() {
        for good in ["joe", "feature/joe", " joe ", "joe/", "/joe", "a.b-c"] {
            assert!(prefix_is_usable(good), "{good:?} was refused");
        }
        for bad in [
            "",
            "   ",
            "//",
            "jo~e",
            "jo^e",
            "jo:e",
            "jo?e",
            "jo*e",
            "jo[e",
            "jo\\e",
            "jo e",
            "jo..e",
            "jo@{e",
            "-joe",
            ".joe",
            "joe.",
            "joe.lock",
            "feature/-joe",
        ] {
            assert!(!prefix_is_usable(bad), "{bad:?} was accepted");
        }
    }

    #[test]
    fn cleaning_a_prefix_cannot_produce_an_empty_component() {
        assert_eq!(clean_prefix("/feature//joe/"), "feature/joe");
        assert_eq!(clean_prefix("  "), "");
    }

    #[test]
    fn the_first_candidate_is_the_bare_name() {
        assert_eq!(candidate("drain-gate", 1), "drain-gate");
        assert_eq!(candidate("drain-gate", 2), "drain-gate-2");
        assert_eq!(candidate("drain-gate", 17), "drain-gate-17");
    }

    #[test]
    fn a_worker_checkout_name_puts_its_slug_below_the_task_id_in_the_branch() {
        assert_eq!(
            branch_component_from_checkout("t-1403-worker-checkout-names-are-ascii-only"),
            "t-1403/worker-checkout-names-are-ascii-only"
        );
        assert_eq!(branch_component_from_checkout("t-1403"), "t-1403");
        assert_eq!(branch_component_from_checkout("drain-gate"), "drain-gate");
    }

    #[test]
    fn a_worker_name_yields_its_ledger_task_id_again() {
        for (name, task) in [
            ("t-1111-went-quiet-fold", "t-1111"),
            ("wt/t-1111-went-quiet-fold", "t-1111"),
            ("wt/t-1111/went-quiet-fold", "t-1111"),
            ("feature/joe/t-9-드레인-게이트-2", "t-9"),
            ("/workspaces/repo/t-42", "t-42"),
        ] {
            assert_eq!(worker_task_id(name), Some(task), "name was {name:?}");
        }
        for unrelated in ["wt/drain-gate", "t-title", "t-", "x-t-9-title", "t-9title"] {
            assert_eq!(worker_task_id(unrelated), None, "name was {unrelated:?}");
        }
    }

    #[test]
    fn collision_suffixes_keep_the_final_name_bounded() {
        let base = "가".repeat(MAX_SLUG_CHARS);
        let last = candidate(&base, 100);
        assert_eq!(last.chars().count(), MAX_WORKTREE_NAME_CHARS);
    }
}
