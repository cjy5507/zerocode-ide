//! The token-probability protocol: one chat request whose reply is one answer
//! mark per question, each mark's probability read off the server's
//! `logprobs` — never a number the model writes.

use judgment_provider::{Protocol, Provider, Refusal, logprobs};
use serde_json::{Value, json};

fn config(address: &str) -> Value {
    json!({"smart":{"judgmentProvider":{
        "kind":"local","protocol":"openai_logprobs","baseUrl":address,
        "model":"acme/decision","requiresAuth":false
    }}})
}

fn provider() -> Provider {
    Provider::from_root(&config("http://127.0.0.1:1234/v1"))
        .expect("a local token-probability provider")
}

fn questions() -> Value {
    json!({
        "destination":{"type":"choice","instructions":"Choose the requested page.","criteria":{"home":"Home","help":"Help"}},
        "urgency":{"type":"score","instructions":"Rate urgency.","criteria":["Routine","Urgent"]},
        "ready":{"type":"noul","instructions":"Is the page ready?","criteria":{"true":"It is ready.","false":"It is not."}}
    })
}

fn request(provider: &Provider, questions: Value) -> Vec<u8> {
    json!({"model":provider.identity(),"state":{"page":"Help","ready":true},"questions":questions})
        .to_string()
        .into_bytes()
}

/// What the prepared request numbered: the questions as the model reads
/// them, in the order their answer marks must come back.
fn asked(body: &[u8]) -> Vec<Value> {
    let sent: Value = serde_json::from_slice(body).expect("JSON request");
    let data: Value =
        serde_json::from_str(sent["messages"][1]["content"].as_str().expect("user data"))
            .expect("decision data");
    data["questions"]
        .as_array()
        .expect("numbered questions")
        .clone()
}

fn token(said: &str, alternatives: &[(&str, f64)]) -> Value {
    let chance = alternatives
        .iter()
        .find(|(alternative, _)| *alternative == said)
        .map_or(0.01, |(_, chance)| *chance);
    json!({
        "token":said,
        "logprob":chance.ln(),
        "top_logprobs":alternatives.iter().map(|(alternative, chance)| json!({"token":alternative,"logprob":chance.ln()})).collect::<Vec<_>>()
    })
}

fn line_break() -> Value {
    token("\n", &[("\n", 0.99)])
}

fn reply(tokens: Vec<Value>, finish: &str) -> Vec<u8> {
    json!({
        "model":"acme/decision",
        "choices":[{"index":0,"finish_reason":finish,"message":{"role":"assistant","content":"marks"},"logprobs":{"content":tokens}}],
        "usage":{"prompt_tokens":120,"completion_tokens":6}
    })
    .to_string()
    .into_bytes()
}

/// The letter the prepared request gave the option named `label`.
fn letter_of(asked: &[Value], label: &str) -> String {
    asked
        .iter()
        .filter_map(|question| question.get("options").and_then(Value::as_object))
        .flat_map(|options| options.iter())
        .find(|(_, option)| option.as_str() == Some(label) || option["option"] == label)
        .map(|(letter, _)| letter.clone())
        .expect("the option was lettered")
}

/// One mark per numbered question: `ready` leans yes, `destination` picks
/// help and `urgency` sits low — each written the way the asked order wants.
fn marks_for(asked: &[Value]) -> Vec<Value> {
    let help = letter_of(asked, "help");
    let home = letter_of(asked, "home");
    let mut tokens = Vec::new();
    for question in asked {
        let mark = if question.get("options").is_some() {
            token(
                &help,
                &[(help.as_str(), 0.88), (home.as_str(), 0.08), ("The", 0.02)],
            )
        } else if question.get("levels").is_some() {
            token("0", &[("0", 0.7), ("1", 0.28), ("Rout", 0.01)])
        } else {
            token(
                "Y",
                &[("Y", 0.8), ("N", 0.15), ("Yes", 0.03), ("The", 0.01)],
            )
        };
        tokens.push(mark);
        tokens.push(line_break());
    }
    tokens
}

#[test]
fn answer_marks_ride_one_chat_request_that_asks_for_token_probabilities() {
    let provider = provider();
    assert!(provider.request_url().ends_with("/v1/chat/completions"));
    let prepared = provider
        .prepare(request(&provider, questions()))
        .expect("decision request");
    let sent: Value = serde_json::from_slice(&prepared.body).expect("JSON request");
    assert_eq!(
        sent["model"], "acme/decision",
        "the server's own model is asked"
    );
    assert_eq!(sent["logprobs"], true);
    assert_eq!(sent["top_logprobs"], logprobs::MAX_CHOICES);
    assert_eq!(sent["temperature"], 0);
    assert_eq!(sent["stream"], false);
    assert_eq!(
        sent["max_tokens"],
        3 * 2 + 8,
        "one mark and one line break per question, and a little spare"
    );
    assert!(
        sent.get("response_format").is_none(),
        "nothing asks the model to write JSON"
    );
    assert!(sent.get("tools").is_none());
    let instructions = sent["messages"][0]["content"]
        .as_str()
        .expect("instructions");
    assert!(instructions.contains("untrusted"));
    assert!(instructions.contains("one line per question"));
    let asked = asked(&prepared.body);
    assert_eq!(asked.len(), 3);
    for (at, question) in asked.iter().enumerate() {
        assert_eq!(
            question["n"],
            at + 1,
            "questions are numbered in the order marks return"
        );
    }
    let choice = asked
        .iter()
        .find(|question| question.get("options").is_some())
        .expect("the choice");
    let letters: Vec<&String> = choice["options"]
        .as_object()
        .expect("options")
        .keys()
        .collect();
    assert_eq!(letters, ["A", "B"]);
    let score = asked
        .iter()
        .find(|question| question.get("levels").is_some())
        .expect("the score");
    assert_eq!(score["levels"], json!({"0":"Routine","1":"Urgent"}));
    let noul = asked
        .iter()
        .find(|question| question["answer"] == "Y or N")
        .expect("the noul");
    assert_eq!(noul["Y"], "It is ready.");
    assert_eq!(noul["N"], "It is not.");
}

#[test]
fn token_probabilities_become_the_system_one_answers_with_their_provenance() {
    let provider = provider();
    let prepared = provider
        .prepare(request(&provider, questions()))
        .expect("request");
    let tokens = marks_for(&asked(&prepared.body));
    let decoded = provider
        .decode(&prepared, reply(tokens, "stop"))
        .expect("a read reply");
    let response: Value = serde_json::from_str(&decoded).expect("JSON answers");
    let ready = response["answers"]["ready"]["noul"].as_f64().expect("noul");
    // Y 0.8 and the written-out "Yes" 0.03 are one mark; N 0.15 the other.
    assert!((ready - 0.83 / 0.98).abs() < 1e-9, "{ready}");
    assert_eq!(response["answers"]["destination"]["choice"], "help");
    let help = response["answers"]["destination"]["probabilities"]["help"]
        .as_f64()
        .expect("help");
    assert!((help - 0.88 / 0.96).abs() < 1e-9, "{help}");
    let urgency = &response["answers"]["urgency"];
    assert!((urgency["score"].as_f64().expect("score") - 0.28 / 0.98).abs() < 1e-9);
    assert_eq!(urgency["legend"], json!({"0":"Routine","1":"Urgent"}));
    assert_eq!(response["judgmentProvider"]["protocol"], "openai_logprobs");
    assert_eq!(
        response["judgmentProvider"]["confidenceSource"],
        "token_probabilities"
    );
    assert_eq!(
        response["judgmentProvider"]["verification"],
        "user_selected_unverified"
    );
    assert_eq!(
        response["usage"],
        json!({"input_tokens":120,"output_tokens":6})
    );
    assert!(
        response["model"]
            .as_str()
            .is_some_and(|model| model.ends_with(":acme/decision")),
        "the answering version is the provider's identity and the server's model"
    );
}

#[test]
fn a_reply_without_token_probabilities_is_refused_by_name() {
    let provider = provider();
    let prepared = provider
        .prepare(request(&provider, questions()))
        .expect("request");
    let bare = json!({
        "model":"acme/decision",
        "choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Y\nA\n0"}}],
        "usage":{"prompt_tokens":120,"completion_tokens":6}
    });
    assert_eq!(
        provider
            .decode(&prepared, bare.to_string().into_bytes())
            .err(),
        Some(Refusal::NoTokenProbabilities)
    );
    let mut empty = marks_for(&asked(&prepared.body));
    empty[0]["top_logprobs"] = json!([]);
    assert_eq!(
        provider.decode(&prepared, reply(empty, "stop")).err(),
        Some(Refusal::NoTokenProbabilities)
    );
    assert_eq!(
        Refusal::NoTokenProbabilities.token(),
        "provider_no_token_probabilities"
    );
}

#[test]
fn a_choice_wider_than_a_tokens_alternatives_is_refused_before_anything_is_sent() {
    let provider = provider();
    let options = |count: usize| -> Value {
        (0..count)
            .map(|at| (format!("page{at}"), json!(format!("Page {at}"))))
            .collect::<serde_json::Map<_, _>>()
            .into()
    };
    let wide = json!({"where":{"type":"choice","instructions":"Pick the page.","criteria":options(logprobs::MAX_CHOICES + 1)}});
    assert_eq!(
        provider.prepare(request(&provider, wide)).err(),
        Some(Refusal::TooManyOptions)
    );
    let widest = json!({"where":{"type":"choice","instructions":"Pick the page.","criteria":options(logprobs::MAX_CHOICES)}});
    assert!(provider.prepare(request(&provider, widest)).is_ok());
    assert_eq!(Refusal::TooManyOptions.token(), "provider_too_many_options");
}

#[test]
fn whitespace_a_tokenizer_glues_on_still_reads_and_a_token_that_is_no_mark_does_not() {
    let provider = provider();
    let ready = json!({"ready":{"type":"noul","instructions":"Is the page ready?"}});
    let prepared = provider
        .prepare(request(&provider, ready.clone()))
        .expect("request");
    let glued = vec![token("\nN", &[("\nN", 0.6), ("\nY", 0.3), (" no", 0.1)])];
    let decoded: Value = serde_json::from_str(
        &provider
            .decode(&prepared, reply(glued, "stop"))
            .expect("read"),
    )
    .expect("JSON");
    assert!((decoded["answers"]["ready"]["noul"].as_f64().expect("noul") - 0.3).abs() < 1e-9);
    let chatty = vec![token("Sure", &[("Sure", 0.7), ("Y", 0.2), ("N", 0.1)])];
    assert_eq!(
        provider.decode(&prepared, reply(chatty, "stop")).err(),
        Some(Refusal::Response),
        "most of the token's weight is on words that are no mark"
    );
}

#[test]
fn a_reply_cut_short_by_its_token_budget_reads_as_truncated() {
    let provider = provider();
    let prepared = provider
        .prepare(request(&provider, questions()))
        .expect("request");
    let mut short = marks_for(&asked(&prepared.body));
    short.truncate(3);
    assert_eq!(
        provider
            .decode(&prepared, reply(short.clone(), "length"))
            .err(),
        Some(Refusal::Truncated)
    );
    assert_eq!(
        provider.decode(&prepared, reply(short, "stop")).err(),
        Some(Refusal::Response)
    );
}

#[test]
fn only_numbers_a_chat_model_writes_are_too_loose_to_carry_an_act_line() {
    assert!(Protocol::SystemOne.calibrated());
    assert!(Protocol::OpenaiLogprobs.calibrated());
    assert!(!Protocol::OpenaiJson.calibrated());
    assert!(
        Protocol::OpenaiLogprobs.chat()
            && Protocol::OpenaiJson.chat()
            && !Protocol::SystemOne.chat()
    );
    assert_eq!(
        serde_json::to_value(Protocol::OpenaiLogprobs).expect("named"),
        "openai_logprobs"
    );
    let remote = json!({"smart":{"judgmentProvider":{
        "kind":"remote","protocol":"openai_logprobs","baseUrl":"https://judge.example.com/v1",
        "model":"acme/decision","requiresAuth":true
    }}});
    let provider = Provider::from_root(&remote).expect("a remote token-probability provider");
    assert_eq!(
        provider.request_url(),
        "https://judge.example.com/v1/chat/completions"
    );
}
