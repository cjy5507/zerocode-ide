//! The one-time code a person hands an agent on the person's-turn card
//! (t-40807).
//!
//! `zerocode-computer handoff --ask-code --reason "…"` puts the usual card in
//! front of the person with one line to type in. What they send comes back as
//! that handoff's answer, once. This module is the whole of what such a card
//! may take, so the window, the manual and the tests read one rule:
//!
//! - **Only a one-time code.** Four to ten letters or digits
//!   ([`OneTimeCode::parse`]); a reason that names a password, a card number,
//!   a security code or the like ([`reason_names_a_secret`], five languages)
//!   gets the card without the line, so the person types that themselves on
//!   their own device, as they always did.
//! - **A value that cannot be written down by accident.** [`OneTimeCode`] is
//!   not `Clone`, `Display` or `Serialize`, and its `Debug` says how long it
//!   is, never what it is: a log line, a record or a panic message that holds
//!   one holds a length.
//! - **One table of limits.** The card reads [`CodeLimits::TABLE`] from the
//!   window; the page keeps no number of its own.

use std::fmt;

use serde::Serialize;

/// The fewest characters a one-time code has. Shorter is a word, an answer,
/// a part of something else.
pub const CODE_MIN_CHARS: usize = 4;
/// The most characters a one-time code has. Longer is a password, a token or
/// a paste of the wrong thing; the codes sites send are six or eight.
pub const CODE_MAX_CHARS: usize = 10;
/// A code is often written in groups (`123 456`, `123-456`): the field takes
/// this many typed characters, separators included, so a full-length code
/// still fits with its spaces.
pub const CODE_TYPED_MAX_CHARS: usize = CODE_MAX_CHARS * 2;
/// What a code written in groups is joined with: the grouping is dropped and
/// the code is the characters between.
const CODE_GROUP_SEPARATORS: [char; 2] = [' ', '-'];

/// What the card tells the page about the line it offers — the window sends
/// this beside the reason, so the page never holds the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLimits {
    pub min: usize,
    pub max: usize,
    pub typed_max: usize,
}

impl CodeLimits {
    pub const TABLE: Self = Self {
        min: CODE_MIN_CHARS,
        max: CODE_MAX_CHARS,
        typed_max: CODE_TYPED_MAX_CHARS,
    };
}

/// Words that name something that outlives one use — a card's number or
/// security code, a PIN, a recovery code, a key, a person's identity number —
/// in the five languages the window speaks. A reason holding one never gets
/// the line, whatever else it says: no word of "one-time" makes a card number
/// a code. A Latin-script word matches as a whole word (so `pin` is not
/// `shipping`), a Hangul, kana or Han word anywhere in the reason (a
/// particle sticks to it).
pub const SECRET_WORDS: &[&str] = &[
    // 한국어
    "카드번호",
    "카드 번호",
    "보안코드",
    "보안 코드",
    "보안카드",
    "핀번호",
    "핀 번호",
    "주민등록번호",
    "주민번호",
    "시드 문구",
    "개인키",
    "복구 코드",
    "복구코드",
    "백업 코드",
    "백업코드",
    // English
    "card number",
    "cvc",
    "cvv",
    "cvv2",
    "security code",
    "pin",
    "pin code",
    "ssn",
    "social security",
    "seed phrase",
    "recovery phrase",
    "private key",
    "secret key",
    "recovery code",
    "backup code",
    // 日本語
    "カード番号",
    "セキュリティコード",
    "暗証番号",
    "マイナンバー",
    "シードフレーズ",
    "秘密鍵",
    "リカバリーコード",
    "バックアップコード",
    // 中文
    "银行卡号",
    "卡号",
    "安全码",
    "助记词",
    "私钥",
    "恢复码",
    "备用码",
    "身份证号",
    // Español
    "número de tarjeta",
    "numero de tarjeta",
    "código de seguridad",
    "codigo de seguridad",
    "frase semilla",
    "clave privada",
    "código de recuperación",
    "codigo de recuperacion",
];

/// Words for a password. A password is a secret — unless the reason says it
/// is a one-time one ([`ONE_TIME_WORDS`]): "일회용 비밀번호", "one-time
/// password" and "ワンタイムパスワード" are what many sites call their codes.
pub const PASSWORD_WORDS: &[&str] = &[
    "비밀번호",
    "비밀 번호",
    "비번",
    "패스워드",
    "암호",
    "password",
    "passcode",
    "passphrase",
    "パスワード",
    "パスコード",
    "密码",
    "contraseña",
    "contrasena",
];

/// Words that say a password is a one-time one.
pub const ONE_TIME_WORDS: &[&str] = &[
    "일회용",
    "1회용",
    "otp",
    "one-time",
    "one time",
    "single-use",
    "single use",
    "ワンタイム",
    "一次性",
    "动态",
    "un solo uso",
    "de un solo uso",
];

/// Whether a handoff's reason asks for something the card must not take — the
/// card is then the plain one, with no line to type in. A secret word always
/// decides it; a password word decides it unless the reason says the password
/// is a one-time one.
#[must_use]
pub fn reason_names_a_secret(reason: &str) -> bool {
    // Runs of whitespace are one space, so "card   number" is "card number".
    let said = reason
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    mentions(&said, SECRET_WORDS)
        || (mentions(&said, PASSWORD_WORDS) && !mentions(&said, ONE_TIME_WORDS))
}

fn mentions(said: &str, words: &[&str]) -> bool {
    words.iter().any(|word| occurs(said, word))
}

/// Whether `word` stands in `said`. A word of the Latin script is a whole word
/// — it may end in an `s` for its plural — so `pin` is not `shipping` and
/// `otp` is not `footprint`; a word of any other script (Hangul, kana, Han)
/// stands wherever it is, because a particle or the next word sticks to it.
fn occurs(said: &str, word: &str) -> bool {
    if !word.chars().all(is_latin_or_space) {
        return said.contains(word);
    }
    said.match_indices(word).any(|(at, _)| {
        let before = said[..at].chars().next_back();
        before.is_none_or(|one| !is_latin(one)) && ends_a_word(&said[at + word.len()..])
    })
}

/// The characters of a word of the Latin script: ASCII letters and digits and
/// the accented letters Spanish writes.
fn is_latin(one: char) -> bool {
    one.is_ascii_alphanumeric() || matches!(one, 'á' | 'é' | 'í' | 'ó' | 'ú' | 'ñ' | 'ü')
}

fn is_latin_or_space(one: char) -> bool {
    is_latin(one) || one == ' ' || one == '-'
}

/// Whether what follows a match ends the word: another script, a space, a
/// mark, the end — or a plural `s` that ends it.
fn ends_a_word(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        None => true,
        Some('s') => chars.next().is_none_or(|one| !is_latin(one)),
        Some(one) => !is_latin(one),
    }
}

/// Why typed text is not a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRefusal {
    Empty,
    NotLettersOrDigits,
    TooShort,
    TooLong,
}

/// A one-time code a person typed: only ever built by [`OneTimeCode::parse`],
/// and never written anywhere. Its `Debug` is its length.
pub struct OneTimeCode(String);

impl OneTimeCode {
    /// What the person typed, as a code — or why it is not one. Spaces and
    /// hyphens between groups are dropped (`123 456` is `123456`); the
    /// characters are ASCII letters and digits, four to ten of them.
    pub fn parse(typed: &str) -> Result<Self, CodeRefusal> {
        // Nothing past the field's own room is read: a paste of a page is
        // refused before it is walked.
        if typed.len() > CODE_TYPED_MAX_CHARS * 4 {
            return Err(CodeRefusal::TooLong);
        }
        let joined: String = typed
            .trim()
            .chars()
            .filter(|one| !CODE_GROUP_SEPARATORS.contains(one))
            .collect();
        if joined.is_empty() {
            return Err(CodeRefusal::Empty);
        }
        if !joined.chars().all(|one| one.is_ascii_alphanumeric()) {
            return Err(CodeRefusal::NotLettersOrDigits);
        }
        if joined.len() < CODE_MIN_CHARS {
            Err(CodeRefusal::TooShort)
        } else if joined.len() > CODE_MAX_CHARS {
            Err(CodeRefusal::TooLong)
        } else {
            Ok(Self(joined))
        }
    }

    /// How many characters the code has — the one thing a record may keep.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }

    /// The code itself: the one place it leaves, for the answer the agent
    /// asked for.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OneTimeCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OneTimeCode({} chars)", self.chars())
    }
}

#[cfg(test)]
mod tests;
