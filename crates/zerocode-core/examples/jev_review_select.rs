//! Compare full-corpus and streaming review acquisition in separate release processes.

use std::hint::black_box;
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::jev::{AGENT_TOOL, door, learning};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "stream".into());
    let count: usize = args.next().map_or(Ok(512), |word| word.parse())?;
    if !matches!(mode.as_str(), "stream" | "whole")
        || !(5..=learning::store::MAX_CASES).contains(&count)
        || args.next().is_some()
    {
        return Err("expected stream or whole, then 5 to 512 cases".into());
    }
    let home = tempfile::tempdir()?;
    let store = learning::store::Store::at(home.path());
    let settings = door::JevSettings::from_root(&json!({"smart":{"jev":{"workspaces":["*"]}}}));
    let workspace = door::resolved_path(home.path());
    let items = vec!["synthetic evidence ".repeat(100); 25];
    let mut source_bytes = 0;
    for at in 0..count {
        let cleared = door::may_send(
            &AGENT_TOOL,
            &door::Asking {
                key: true,
                settings: &settings,
                workspace: Some(&workspace),
                sent_today: 0,
            },
            json!({"state":{"question":format!("case {at}"),"items":items},
                "questions":{"q":{"type":"noul","instructions":"Are these items useful?"}}}),
        )
        .map_err(|refusal| refusal.token())?;
        let case = learning::Case::from_cleared(
            &AGENT_TOOL,
            &cleared,
            &json!({"model":"jev-bench","answers":{"q":{"type":"noul","noul":0.5}}}),
            i64::try_from(at)?,
        )?;
        source_bytes += serde_json::to_vec(&case)?.len();
        if store.record(&case)? != learning::store::Recorded::Saved {
            return Err("a synthetic case was not retained".into());
        }
    }
    let began = Instant::now();
    let (selected, counted, invalid): (Vec<String>, usize, usize) = if mode == "stream" {
        let review = store.review(None, None, 5, 1, 42)?;
        let ids = review
            .samples
            .iter()
            .map(|sample| sample.case.id.clone())
            .collect();
        black_box(&review);
        (ids, review.cases, review.invalid)
    } else {
        let snapshot = store.snapshot()?;
        let samples = learning::select(&snapshot.cases, &snapshot.outcomes, 5, 1, 42)?;
        let ids = samples
            .iter()
            .map(|sample| sample.case.id.clone())
            .collect();
        black_box(&snapshot);
        (ids, snapshot.cases.len(), snapshot.invalid)
    };
    let elapsed = began.elapsed();
    if counted != count || invalid != 0 || selected.len() != 5 {
        return Err("acquisition lost a valid case".into());
    }
    let result: Value = json!({"mode":mode,"cases":counted,"selected":selected,
        "sourceBytes":source_bytes,"elapsedMicros":elapsed.as_micros()});
    println!("{result}");
    Ok(())
}
