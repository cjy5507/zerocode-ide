//! The long-form bench's door to the core's choice of a line by its words
//! (t-37883): the fake desk hands it what a `readText --ocr` answered and a
//! press's words on stdin, and it prints the line chosen — the rule the window
//! runs (`computer_use_protocol::words`), so the bench keeps no copy of it.
//!
//! ```text
//! echo '{"answer":{"lines":[…]},"text":"No","after":"visited a farm"}' \
//!   | cargo run -q -p zerocode-core --example words_choose
//! {"ok":true,"text":"No","x":1070.0,"y":428.5}
//! ```

use std::io::Read as _;

use serde_json::{Value, json};
use zerocode_core::computer_use_protocol::words;

fn main() {
    let mut input = String::new();
    let asked: Value = std::io::stdin()
        .read_to_string(&mut input)
        .ok()
        .and_then(|_| serde_json::from_str(&input).ok())
        .unwrap_or_default();
    let lines = words::lines(&asked["answer"]);
    let words = asked["text"].as_str().unwrap_or_default();
    let answer = match words::choose(&lines, words, asked["after"].as_str()) {
        Ok(line) => {
            let (x, y) = line.center();
            json!({ "ok": true, "text": line.text, "x": x, "y": y })
        }
        Err(refusal) => json!({ "ok": false, "code": refusal.code, "message": refusal.message }),
    };
    println!("{answer}");
}
