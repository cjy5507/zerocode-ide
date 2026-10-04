//! The one-time code a person hands an agent on the person's-turn card
//! (t-40807).
//!
//! `zerocode-computer handoff --ask-code --reason "…" --into '[…]'` puts the
//! usual card in front of the person with one line to type in. What they send
//! never comes back to the agent: the window types it, once, into the field
//! the agent named in `--into` ([`EnterInto`]), and the answer says how many
//! characters went in. This module is the whole of what such a card may take
//! and where its code may go, so the window, the manual and the tests read one
//! rule:
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
/// The most bytes typed text may have before it is refused unread: the
/// field's own room, at the four bytes a character takes at most in UTF-8.
const TYPED_MAX_BYTES: usize = CODE_TYPED_MAX_CHARS * 4;

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
        if typed.len() > TYPED_MAX_BYTES {
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

/// The characters a code is typed as while a command that would carry it is
/// checked: any four-character code, so a checked command never holds the
/// person's own.
pub const ENTER_PLACEHOLDER: &str = "0000";

/// The door an input to a field comes in by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterTool {
    /// The desktop: an app's element, through the accessibility tree.
    Computer,
    /// A tab of the window's own browser.
    Browser,
    /// A phone in the window's emulator pane.
    Emulator,
}

impl EnterTool {
    /// The word a door is told by in an answer.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Computer => "computer",
            Self::Browser => "browser",
            Self::Emulator => "emulator",
        }
    }
}

/// Words an input for a code may not carry: a value of its own (the code is
/// the value, and only the window writes it), an answer's shape, a press the
/// person is to confirm, and the guard on ZeroCode's own window.
const ENTER_REFUSED_WORDS: [&str; 8] = [
    "--value",
    "--text",
    "--value-stdin",
    "--text-stdin",
    "--json",
    "--confirming",
    "--confirm",
    "--allow-self",
];

/// What `--into` may name, said once and carried by every refusal that needs
/// to say it.
const ENTER_SHAPES: &str = "set-value (--app, --element-index), type-text (--app), browser type <tab> <selector>, or emulator text (--platform, --device)";

/// Where the window puts a code the person typed: the words of one input
/// command, without the code. `["set-value","--app","Form","--element-index",
/// "3"]` for an app's field, `["type-text","--app","Form"]` for its focused
/// field, `["browser","type","browser-1","#otp"]` for a tab's, `["emulator",
/// "text","--platform","ios","--device","<id>"]` for a phone's. Checked whole
/// before the card is shown ([`EnterInto::from_words`]), so a code is never
/// asked for that has nowhere to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterInto {
    tool: EnterTool,
    /// The words as the agent gave them, door prefix (`browser`, `emulator`)
    /// and all.
    given: Vec<String>,
}

impl EnterInto {
    /// The words of the flag, as the agent wrote them: a JSON array of strings.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let words: Vec<String> = serde_json::from_str(raw).map_err(|_| {
            "--into is a JSON array of the words of one input command, e.g. '[\"set-value\",\"--app\",\"Form\",\"--element-index\",\"3\"]'".to_string()
        })?;
        Self::from_words(&words)
    }

    /// The command's words, checked: only an input that puts a typed value in
    /// one field, naming its field, and no word that would carry a value, a
    /// second command or a way past the person. The same parsers that read a
    /// command carrying a value read this one with a placeholder for it.
    pub fn from_words(all: &[String]) -> Result<Self, String> {
        let Some(first) = all.first() else {
            return Err("--into needs the words of one input command".to_string());
        };
        if all.iter().any(String::is_empty) {
            return Err("--into holds an empty word".to_string());
        }
        let tool = match first.as_str() {
            "browser" => EnterTool::Browser,
            "emulator" => EnterTool::Emulator,
            _ => EnterTool::Computer,
        };
        let into = Self {
            tool,
            given: all.to_vec(),
        };
        match tool {
            EnterTool::Computer => into.check_computer()?,
            EnterTool::Browser => into.check_browser()?,
            EnterTool::Emulator => into.check_emulator()?,
        }
        Ok(into)
    }

    /// The door the input comes in by.
    #[must_use]
    pub const fn tool(&self) -> EnterTool {
        self.tool
    }

    /// The words after the door's prefix: the input command itself.
    fn words(&self) -> &[String] {
        match self.tool {
            EnterTool::Computer => &self.given,
            EnterTool::Browser | EnterTool::Emulator => &self.given[1..],
        }
    }

    /// The input's verb: `set-value`, `type-text`, `type` or `text`.
    #[must_use]
    pub fn verb(&self) -> &str {
        self.words().first().map_or("", String::as_str)
    }

    /// The words the door takes to type `value`: the command with the value
    /// where its verb takes one.
    #[must_use]
    pub fn words_with(&self, value: &str) -> Vec<String> {
        let mut words = self.words().to_vec();
        match (self.tool, self.verb()) {
            (EnterTool::Computer, "set-value") => words.push("--value".to_string()),
            (EnterTool::Computer, _) | (EnterTool::Emulator, _) => words.push("--text".to_string()),
            (EnterTool::Browser, _) => {}
        }
        words.push(value.to_string());
        words
    }

    /// The array as the agent gave it, door prefix and all.
    #[must_use]
    pub fn to_words(&self) -> Vec<String> {
        self.given.clone()
    }

    /// No word of `words` that would carry a value of its own, or a way past
    /// the person.
    fn refuse_words(words: &[String]) -> Result<(), String> {
        match words
            .iter()
            .find_map(|word| ENTER_REFUSED_WORDS.iter().find(|refused| **refused == word))
        {
            Some(refused) => Err(format!(
                "--into may not carry {refused}: the window types the code as the value, and nothing of it comes back"
            )),
            None => Ok(()),
        }
    }

    fn check_computer(&self) -> Result<(), String> {
        let words = self.words();
        let verb = words[0].as_str();
        let needs: &[&str] = match verb {
            "set-value" => &["--app", "--element-index"],
            "type-text" => &["--app"],
            "paste-text" | "clipboard-write" => {
                return Err(format!(
                    "`{verb}` goes by the clipboard, where a code stays for `clipboard-read`: use {ENTER_SHAPES}"
                ));
            }
            other => {
                return Err(format!(
                    "`{other}` is not an input a code can go to: use {ENTER_SHAPES}"
                ));
            }
        };
        Self::refuse_words(&words[1..])?;
        for need in needs {
            if !words.iter().any(|word| word == need) {
                return Err(format!(
                    "{verb} needs {need} to say which field the code goes in"
                ));
            }
        }
        let typed = self.words_with(ENTER_PLACEHOLDER);
        let parsed = crate::computer_use::parse_command(&typed)?;
        let method = match verb {
            "set-value" => crate::computer_use::ComputerMethod::SetValue,
            _ => crate::computer_use::ComputerMethod::TypeText,
        };
        if parsed.method == method {
            Ok(())
        } else {
            Err(format!(
                "`{verb}` is not an input a code can go to: use {ENTER_SHAPES}"
            ))
        }
    }

    fn check_browser(&self) -> Result<(), String> {
        let words = self.words();
        let shaped = "--into for the browser is [\"browser\",\"type\",\"<tab>\",\"<selector>\"] — no text: the window types the code (a selector may be a handle `fields` printed)";
        let (words, setter) = match words.last().map(String::as_str) {
            Some(crate::agent_browser::TYPE_VALUE_FLAG) if words.len() == 4 => (&words[..3], true),
            _ => (words, false),
        };
        Self::refuse_words(words)?;
        if words.len() != 3 || words[0] != "type" {
            return Err(shaped.to_string());
        }
        let mut typed = words.to_vec();
        if setter {
            typed.push(crate::agent_browser::TYPE_VALUE_FLAG.to_string());
        }
        typed.push(ENTER_PLACEHOLDER.to_string());
        crate::agent_browser::arity_ok(&typed).map_err(|why| format!("{shaped}: {why}"))
    }

    fn check_emulator(&self) -> Result<(), String> {
        let words = self.words();
        let shaped = "--into for a phone is [\"emulator\",\"text\",\"--platform\",\"<ios|android>\",\"--device\",\"<id>\"] — no --text: the window types the code";
        Self::refuse_words(words)?;
        if words.first().map(String::as_str) != Some("text") {
            return Err(shaped.to_string());
        }
        let typed = self.words_with(ENTER_PLACEHOLDER);
        let parsed = crate::computer_use::parse_emulator_command(&typed)
            .map_err(|why| format!("{shaped}: {why}"))?;
        if parsed.platform.is_none() || parsed.device.is_none() {
            return Err(format!(
                "{shaped}: it needs --platform and --device to say which phone"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
