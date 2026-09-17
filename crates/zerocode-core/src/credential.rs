//! What a credential looks like in a line of text — one table, for every road
//! that must not carry one onward.
//!
//! Two roads read it, with two policies over the same words. A crash
//! breadcrumb (the window's `crumbs`) is written for strangers who may read a
//! report, so it masks broadly: a word that NAMES a credential takes the rest
//! of its line with it. A handover recap (`crate::orchestration`) is written
//! for the next agent on the same task, so it masks VALUES only — "added
//! password validation" is a sentence the replacement needs, `DB_PASSWORD=…`
//! is a value it must not be handed, least of all by another provider's model.
//! Both ask this module which words are which; neither keeps a list of its own.
//!
//! The three the crash road calls from a panic hook — [`names_a_credential`],
//! [`looks_like_a_credential`] and [`contains_ascii_case`] — allocate
//! nothing; [`mask_values`] and [`may_carry_a_credential`] build strings.

/// Words that NAME a credential, matched inside a word in any ASCII case.
pub const KEY_WORDS: [&str; 7] = [
    "authorization",
    "cookie",
    "token",
    "password",
    "secret",
    "api_key",
    "apikey",
];

/// Marks a credential VALUE carries: vendor token shapes and a JWT's head.
pub const VALUE_MARKS: [&str; 15] = [
    "sk-",
    "sk_",
    "AKIA",
    "ASIA",
    "AIza",
    "glpat-",
    "npm_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "ghp_",
    "github_pat_",
    "xox",
    "eyJ",
];

/// What a masked value reads as.
pub const MASK: &str = "[redacted]";

/// How long a mixed alphanumeric word must be before it reads as a token.
const OPAQUE_WORD_MIN_BYTES: usize = 24;

/// Whether `word` names a credential: one of [`KEY_WORDS`] inside it.
#[must_use]
pub fn names_a_credential(word: &str) -> bool {
    KEY_WORDS.iter().any(|key| contains_ascii_case(word, key))
}

/// Whether `word` looks like a credential value: one of [`VALUE_MARKS`]
/// inside it, or a long mixed alphanumeric word that is neither a symbol path
/// (`tao::platform_impl::…`, which a backtrace is made of) nor a plain hex
/// digest (a commit sha, a content hash).
#[must_use]
pub fn looks_like_a_credential(word: &str) -> bool {
    VALUE_MARKS
        .iter()
        .any(|mark| contains_ascii_case(word, mark))
        || (word.len() >= OPAQUE_WORD_MIN_BYTES
            && !word.contains("::")
            && word.bytes().any(|byte| byte.is_ascii_digit())
            && word.bytes().any(|byte| byte.is_ascii_alphabetic())
            && !word.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Whether `needle` occurs in `raw`, ASCII case ignored. No allocation.
#[must_use]
pub fn contains_ascii_case(raw: &str, needle: &str) -> bool {
    raw.as_bytes()
        .windows(needle.len())
        .any(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Auth-scheme words whose next word is the credential (`Bearer <token>`).
/// `Basic` is not here: it is an ordinary English word, and a Basic header
/// already names itself (`Authorization: Basic …`).
const SCHEME_WORDS: [&str; 1] = ["bearer"];

/// Programs that take a password glued to a short flag (`mysql -phunter2`),
/// and that flag.
const GLUED_SECRET_FLAGS: [(&str, &str); 2] = [("mysql", "-p"), ("mariadb", "-p")];

/// A `.netrc` line (`machine h login u password p`) starts with one of these.
const NETRC_LEADS: [&str; 2] = ["machine", "default"];

/// `text` with credential VALUES masked, its lines and spacing kept:
/// - a word that looks like a value ([`looks_like_a_credential`]) is masked;
/// - `KEY=value` and `KEY:value` whose key names a credential keep the key
///   and lose the value;
/// - a word that names a credential and ends with `:` (`Authorization:`)
///   masks the rest of its line — a header's value can hold spaces;
/// - a flag that names one (`--password`, `-token`) masks the word after it,
///   and so does an auth scheme (`Bearer`) and a `.netrc` `password`;
/// - a masked value that opens a quote is masked to its closing quote
///   (`DB_PASSWORD='alpha beta'`), however many words it spans;
/// - a password glued to its flag (`mysql -phunter2`) loses the password;
/// - a URL's userinfo (`https://user:token@host`) loses everything before `@`
///   ([`crate::clone::scrub_credentials`]).
///
/// A word that merely names a credential in prose is left alone. What this
/// cannot see — a value under a name that says nothing (`export X=hunter2`)
/// — is why a road that must be sure asks [`may_carry_a_credential`] and
/// withholds the line instead.
#[must_use]
pub fn mask_values(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        mask_line(&crate::clone::scrub_credentials(line), &mut out);
    }
    out
}

/// Whether `line` carries anything that is, or could introduce, a
/// credential: a word that names or looks like one, an auth scheme, a URL's
/// userinfo, a glued password flag, a shell `export`, or a PEM block. A road
/// that must not pass a credential on withholds such a line whole rather than
/// trust [`mask_values`] to find the whole value. No allocation beyond the
/// userinfo check.
#[must_use]
pub fn may_carry_a_credential(line: &str) -> bool {
    names_a_credential(line)
        || line.split_whitespace().any(looks_like_a_credential)
        || SCHEME_WORDS
            .iter()
            .any(|scheme| contains_ascii_case(line, scheme))
        || glued_secret_flag(line).is_some()
        || contains_ascii_case(line, "export ")
        || contains_ascii_case(line, "-----BEGIN")
        || crate::clone::scrub_credentials(line) != line
}

/// The flag a password is glued to on this line, when the line runs one of
/// [`GLUED_SECRET_FLAGS`]' programs and a word carries that flag with more
/// after it.
fn glued_secret_flag(line: &str) -> Option<&'static str> {
    GLUED_SECRET_FLAGS
        .iter()
        .find(|(program, flag)| {
            contains_ascii_case(line, program)
                && line
                    .split_whitespace()
                    .any(|word| word.len() > flag.len() && word.starts_with(flag))
        })
        .map(|(_, flag)| *flag)
}

/// The quote a masked value opens and does not close within `value`.
fn unclosed_quote(value: &str) -> Option<char> {
    let quote = value
        .chars()
        .next()
        .filter(|glyph| *glyph == '\'' || *glyph == '"')?;
    (!value[quote.len_utf8()..].contains(quote)).then_some(quote)
}

fn mask_line(line: &str, out: &mut String) {
    let netrc = line
        .split_whitespace()
        .next()
        .is_some_and(|first| NETRC_LEADS.contains(&first));
    let glued = glued_secret_flag(line);
    let mut mask_next = false;
    // Inside a masked value that opened a quote: every word up to the one
    // that closes it is part of the value.
    let mut quoted: Option<char> = None;
    let mut rest = line;
    while !rest.is_empty() {
        let space = rest.len() - rest.trim_start().len();
        let spacing = &rest[..space];
        rest = &rest[space..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        rest = after;
        if let Some(quote) = quoted {
            if word.contains(quote) {
                quoted = None;
            }
            continue;
        }
        out.push_str(spacing);
        if word.is_empty() {
            continue;
        }
        if mask_next || looks_like_a_credential(word) {
            out.push_str(MASK);
            quoted = unclosed_quote(word);
            mask_next = false;
            continue;
        }
        if let Some(split) = word.find(['=', ':'])
            && split + 1 < word.len()
            && names_a_credential(&word[..split])
        {
            out.push_str(&word[..=split]);
            out.push_str(MASK);
            quoted = unclosed_quote(&word[split + 1..]);
            continue;
        }
        if let Some(flag) = glued
            && word.len() > flag.len()
            && word.starts_with(flag)
        {
            out.push_str(flag);
            out.push_str(MASK);
            continue;
        }
        let bare = word.trim_matches(|glyph: char| glyph == '"' || glyph == '\'');
        if names_a_credential(bare) && bare.ends_with(':') {
            out.push_str(word);
            if !rest.trim().is_empty() {
                out.push(' ');
                out.push_str(MASK);
            }
            return;
        }
        if (bare.starts_with('-') && names_a_credential(bare))
            || SCHEME_WORDS
                .iter()
                .any(|scheme| bare.eq_ignore_ascii_case(scheme))
            || (netrc && bare == "password")
        {
            mask_next = true;
        }
        out.push_str(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_masked_and_their_keys_and_the_prose_around_them_stay() {
        for (said, kept, gone) in [
            (
                "export DB_PASSWORD=hunter2 && make",
                "DB_PASSWORD=",
                "hunter2",
            ),
            ("OPENAI_API_KEY=sk-proj-abc123 zo", "zo", "sk-proj-abc123"),
            (
                "curl -H 'Authorization: Bearer abc.def' x",
                "'Authorization:",
                "abc.def",
            ),
            ("gh --token ghx12345 pr list", "--token", "ghx12345"),
            (
                "aws AKIAIOSFODNN7EXAMPLE s3 ls",
                "s3 ls",
                "AKIAIOSFODNN7EXAMPLE",
            ),
            ("set cookie:session=abc; ok", "cookie:", "session=abc;"),
            ("jwt eyJhbGciOi.x.y here", "here", "eyJhbGciOi.x.y"),
            (
                "key Xk9pQ2mZ7vL4nR8sT1wY6bC3dF5g",
                "key",
                "Xk9pQ2mZ7vL4nR8sT1wY6bC3dF5g",
            ),
            // Review t-3717 B-1: a quoted value is masked to its closing
            // quote, a scheme word and a .netrc password take the next word,
            // and a password glued to mysql's flag loses the password.
            (
                "export DB_PASSWORD='alpha beta' && make",
                "DB_PASSWORD=[redacted] && make",
                "beta",
            ),
            (
                "client --password \"alpha beta gamma\" next",
                "--password [redacted] next",
                "gamma",
            ),
            (
                "curl -H 'Bearer abc.def' x",
                "'Bearer [redacted] x",
                "abc.def",
            ),
            (
                "machine h login u password hunter2",
                "login u password [redacted]",
                "hunter2",
            ),
            (
                "mysql -phunter2 -u root",
                "mysql -p[redacted] -u root",
                "hunter2",
            ),
        ] {
            let masked = mask_values(said);
            assert!(masked.contains(kept), "{said:?} lost {kept:?}: {masked:?}");
            assert!(!masked.contains(gone), "{said:?} kept {gone:?}: {masked:?}");
            assert!(masked.contains(MASK), "{said:?}: {masked:?}");
        }
    }

    #[test]
    fn a_urls_userinfo_is_scrubbed_and_its_host_stays() {
        let masked = mask_values("git clone https://user:tok3n@host.test/r.git now");
        assert_eq!(masked, "git clone https://***@host.test/r.git now");
    }

    /// A road that must be sure withholds a line with any of these cues —
    /// including the ones no mask can see (`export X=hunter2`) — and keeps
    /// the ordinary tool calls a replacement needs.
    #[test]
    fn a_line_that_may_carry_a_credential_is_told_from_an_ordinary_call() {
        for risky in [
            "Bash · export X=hunter2",
            "Bash · curl -H 'Bearer abc' https://api.test",
            "Bash · git push https://u:p@host.test/r.git",
            "Bash · mysql -phunter2 app",
            "Bash · cat <<EOF\n-----BEGIN OPENSSH PRIVATE KEY-----",
            "Bash · aws s3 ls --profile AKIAIOSFODNN7EXAMPLE",
            "Edit · crates/auth/src/token_store.rs",
        ] {
            assert!(may_carry_a_credential(risky), "{risky:?}");
        }
        for ordinary in [
            "Bash · cargo test -p parser",
            "Edit · crates/zerocode-core/src/untrusted.rs",
            "Read · docs/design/x.md:1-40",
            "Grep · fn main in src",
            "exec_command · git status --short",
        ] {
            assert!(!may_carry_a_credential(ordinary), "{ordinary:?}");
        }
    }

    #[test]
    fn a_word_that_only_names_a_credential_in_prose_is_left_alone() {
        for prose in [
            "Added password validation to the login form",
            "the tokenizer now keeps the secret sauce private",
            "Edit · crates/auth/src/token_store.rs",
            "commit 9f2c4e1a7b3d5e6f8a9b0c1d2e3f4a5b6c7d8e9f",
            "tao::platform_impl::platform::window::send_event::h7f78a2b3f8b21b08",
            "cargo test -p zerocode-core --lib untrusted",
        ] {
            assert_eq!(mask_values(prose), prose);
        }
    }

    #[test]
    fn lines_and_spacing_are_kept() {
        let said = "first  line\n\tpassword: hunter2 more\nthird";
        assert_eq!(
            mask_values(said),
            "first  line\n\tpassword: [redacted]\nthird"
        );
    }
}
