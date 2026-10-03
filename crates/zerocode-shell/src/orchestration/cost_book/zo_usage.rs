use std::collections::BTreeSet;
use std::io::Read;

use zerocode_core::orchestration::Ledger;
use zerocode_core::orchestration::task_cost::{SessionBook, ZoRequestUsage, reported_sessions};

pub(super) fn read(ledger: &Ledger, sessions: &mut SessionBook, now_ms: i64) {
    sessions.clear_source(zerocode_core::orchestration::task_cost::UsageSource::Zo);
    let Some(settings) = crate::api_routers::zo_settings_path() else {
        return;
    };
    let Some(home) = settings.parent() else {
        return;
    };
    let mut ids = BTreeSet::new();
    for run in ledger.runs() {
        for attempt in &run.dispatches {
            if run
                .worker(&attempt.worker)
                .is_none_or(|worker| worker.agent != "zo")
            {
                continue;
            }
            for session in reported_sessions(run, attempt) {
                if session.id.is_empty()
                    || !session
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                    || !ids.insert(session.id.clone())
                {
                    continue;
                }
                let file = home
                    .join("cache/prompt-cache")
                    .join(&session.id)
                    .join("requests.jsonl");
                let Some(rows) = read_rows(&file) else {
                    continue;
                };
                sessions.read_zo_usage(&session.id, &rows, now_ms);
            }
        }
    }
}

fn read_rows(file: &std::path::Path) -> Option<Vec<ZoRequestUsage>> {
    let cap = 16 * 1024 * 1024;
    let file = crate::durable_file::open_plain_file(file).ok()?;
    if file.metadata().ok()?.len() > cap {
        return None;
    }
    let mut contents = String::new();
    file.take(cap + 1).read_to_string(&mut contents).ok()?;
    if contents.len() as u64 > cap {
        return None;
    }
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .ok()
}
