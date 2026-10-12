use super::*;
use std::collections::BTreeMap;
use zerocode_core::orchestration::gate_reuse::{self as gate, JobIntent, JobMode, JobScope};

const SESSION_CAP: usize = 32;
pub const FRAME_CAP: usize = 32 * 1024;

pub fn open_native(
    actor: &std::sync::Arc<RuntimeActor>,
    team: &str,
    pane: &str,
    capability: &str,
    caller: Option<String>,
    body: &str,
) -> Result<serde_json::Value, &'static str> {
    if cfg!(not(unix)) {
        return Err("unsupported_platform");
    }
    if body.len() > 1024 * 1024 {
        return Err("invalid_request");
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| "invalid_request")?;
    let object = value.as_object().ok_or("invalid_request")?;
    if object.len() != 3
        || object
            .keys()
            .any(|key| !["intent", "manifest", "bootstrap"].contains(&key.as_str()))
    {
        return Err("unknown_field");
    }
    let intent = parsed(&value["intent"]).map_err(|reason| code(&reason))?;
    let bootstrap = parsed(&value["bootstrap"]).map_err(|reason| code(&reason))?;
    let manifest = value["manifest"]
        .as_str()
        .filter(|text| text.len() <= 1024 * 1024)
        .ok_or("invalid_request")?;
    let owner = PlanCommand::checked(
        vec!["native-gate-open".into()],
        team,
        pane,
        capability,
        caller,
        wall_time(0),
    )
    .map_err(|_| "authority_unavailable")?;
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| "authority_unavailable")?;
    let port = listener
        .local_addr()
        .map_err(|_| "authority_unavailable")?
        .port();
    let result = actor
        .open_gate_session(owner, intent, manifest.into(), bootstrap)
        .map_err(|_| "authority_unavailable")?;
    if result["ok"] != true {
        return Err(code(result["code"].as_str().unwrap_or_default()));
    }
    let job = result["value"]["job"]
        .as_str()
        .ok_or("authority_unavailable")?;
    let Some(expires_ms) = result["value"]["expiresMs"].as_i64() else {
        let _ = actor.close_gate_session(job.into());
        return Err("authority_unavailable");
    };
    serve(
        std::sync::Arc::downgrade(actor),
        listener,
        job.into(),
        expires_ms,
    );
    Ok(serde_json::json!({"ok":true,"job":job,"port":port,"expiresMs":expires_ms}))
}

pub enum Request {
    Open {
        owner: Box<PlanCommand>,
        intent: Box<JobIntent>,
        manifest: String,
        bootstrap: Box<gate::BootstrapIdentity>,
    },
    Bind {
        job: String,
        capability: String,
        now_ms: i64,
    },
    Command {
        job: String,
        body: serde_json::Value,
        now_ms: i64,
    },
    Alive {
        job: String,
        now_ms: i64,
    },
    Close {
        job: String,
    },
    #[cfg(test)]
    ExpireDeadline {
        job: String,
    },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Open { .. } => "GateSession::Open",
            Self::Bind { .. } => "GateSession::Bind",
            Self::Command { .. } => "GateSession::Command",
            Self::Alive { .. } => "GateSession::Alive",
            Self::Close { .. } => "GateSession::Close",
            #[cfg(test)]
            Self::ExpireDeadline { .. } => "GateSession::ExpireDeadline",
        })
    }
}

pub fn serve(
    actor: std::sync::Weak<RuntimeActor>,
    listener: std::net::TcpListener,
    job: String,
    expires_ms: i64,
) {
    std::thread::spawn(move || {
        struct Close {
            actor: std::sync::Weak<RuntimeActor>,
            job: String,
        }
        impl Drop for Close {
            fn drop(&mut self) {
                if let Some(actor) = self.actor.upgrade() {
                    let _ = actor.close_gate_session(self.job.clone());
                }
            }
        }
        let _close = Close {
            actor: actor.clone(),
            job: job.clone(),
        };
        let accepted = (|| -> std::io::Result<std::net::TcpStream> {
            listener.set_nonblocking(true)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((stream, peer)) if peer.ip().is_loopback() => return Ok(stream),
                    Ok(_) => return Err(std::io::ErrorKind::PermissionDenied.into()),
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        let Ok(mut stream) = accepted else {
            return;
        };
        drop(listener);
        if stream.set_nonblocking(false).is_err()
            || stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .is_err()
            || stream
                .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                .is_err()
        {
            return;
        }
        let capability = (|| -> std::io::Result<String> {
            let value = read_frame(&mut stream)?.ok_or(std::io::ErrorKind::TimedOut)?;
            let object = value.as_object().ok_or(std::io::ErrorKind::InvalidData)?;
            if object.len() != 1 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            object
                .get("capability")
                .and_then(serde_json::Value::as_str)
                .filter(|value| value.len() <= MAX_TOKEN_BYTES)
                .map(str::to_string)
                .ok_or(std::io::ErrorKind::InvalidData.into())
        })();
        let Ok(capability) = capability else {
            return;
        };
        let Some(held) = actor.upgrade() else {
            return;
        };
        let bound = held.bind_gate_session(job.clone(), capability, wall_time(0));
        drop(held);
        let Ok(bound) = bound else {
            return;
        };
        if write_frame(&mut stream, &bound).is_err() || bound["ok"] != true {
            return;
        }
        loop {
            let at = wall_time(0);
            let Some(held) = actor.upgrade() else {
                return;
            };
            if at >= expires_ms || held.gate_session_alive(job.clone(), at).ok() != Some(true) {
                return;
            }
            drop(held);
            match read_frame(&mut stream) {
                Ok(Some(body)) => {
                    let Some(held) = actor.upgrade() else {
                        return;
                    };
                    let reply = held.command_gate_session(job.clone(), body, wall_time(0));
                    drop(held);
                    let Ok(reply) = reply else {
                        return;
                    };
                    if write_frame(&mut stream, &reply).is_err() || reply["ok"] != true {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    });
}

pub fn read_frame(stream: &mut impl std::io::Read) -> std::io::Result<Option<serde_json::Value>> {
    let mut length = [0_u8; 4];
    match stream.read(&mut length[..1]) {
        Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    stream.read_exact(&mut length[1..])?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > FRAME_CAP {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|_| std::io::ErrorKind::InvalidData.into())
}

pub fn write_frame(
    stream: &mut impl std::io::Write,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).map_err(|_| std::io::ErrorKind::InvalidData)?;
    if body.is_empty() || body.len() > FRAME_CAP {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)
}

pub(super) struct Jobs {
    rows: BTreeMap<String, Session>,
    next: u64,
    epoch: String,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
            next: 0,
            epoch: random_instance_digest(),
        }
    }
}

impl Jobs {
    pub(super) fn clear(&mut self) {
        self.rows.clear();
    }
}

#[derive(PartialEq, Eq)]
enum Stage {
    Initial,
    Registered,
    Started,
}

struct Session {
    namespace: String,
    deadline: std::time::Instant,
    owner: PlanCommand,
    scope: JobScope,
    bound: bool,
    stage: Stage,
    registration: Option<String>,
    lease: Option<String>,
    register_retry: Option<String>,
}

fn wall_time(at: i64) -> i64 {
    let current = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(i64::MAX);
    at.max(current)
}

fn code(reason: &str) -> &'static str {
    match reason.split(':').next().unwrap_or_default() {
        "unpinned" => "unpinned",
        "digest_mismatch" => "digest_mismatch",
        "stale_pin" => "stale_pin",
        "unknown_field" => "unknown_field",
        "expired" => "expired",
        "out_of_scope" => "out_of_scope",
        "invalid_request" => "invalid_request",
        _ => "authority_unavailable",
    }
}

fn answer(ok: bool, value: serde_json::Value, reason: &str, revision: u64) -> RuntimeReply {
    RuntimeReply::GateSession {
        value: serde_json::json!({"ok":ok,"value":value,"code":if ok { "ok" } else { code(reason) }}),
        revision,
    }
}

fn parsed<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| {
        if error.to_string().contains("unknown field") {
            "unknown_field: gate request".into()
        } else {
            "invalid_request: gate request".into()
        }
    })
}

impl RuntimeState {
    fn gate_authorized(&self, session: &Session, at: i64) -> Result<(), String> {
        if std::time::Instant::now() >= session.deadline {
            return Err("expired: gate job deadline".into());
        }
        let mut checked = Err("authority_unavailable: gate owner".into());
        self.panes.with_pane_authority(
            session.owner.team(),
            session.owner.pane(),
            session.owner.presented(),
            &mut |team| {
                if let Some(team) = team {
                    checked = gate::validate_job(
                        &self.ledger,
                        team,
                        session.owner.pane(),
                        session.owner.actor().unwrap_or_default(),
                        &session.scope,
                        at,
                    )
                    .and_then(|()| {
                        match session.registration.as_deref().or(session
                            .scope
                            .intent
                            .registration
                            .as_deref())
                        {
                            Some(id) => {
                                gate::validate_job_receipt(&self.ledger, &session.scope, id)
                            }
                            None => Ok(()),
                        }
                    });
                }
            },
        );
        checked
    }

    pub(super) fn prune_gate_jobs(&mut self) {
        if self.gate_jobs.rows.is_empty() {
            return;
        }
        let at = wall_time(0);
        let expired = self
            .gate_jobs
            .rows
            .iter()
            .filter(|(_, session)| self.gate_authorized(session, at).is_err())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.gate_jobs.rows.remove(&id);
        }
    }

    pub(super) fn gate_request(&mut self, request: Request) -> Result<RuntimeReply, RuntimeError> {
        if !self.recovery_permits.is_empty() {
            self.gate_jobs.rows.clear();
            return Err(RuntimeError::RecoveryRequired);
        }
        match request {
            #[cfg(test)]
            Request::ExpireDeadline { job } => {
                let Some(session) = self.gate_jobs.rows.get_mut(&job) else {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope",
                        self.revision,
                    ));
                };
                session.deadline = std::time::Instant::now();
                Ok(answer(true, serde_json::Value::Null, "", self.revision))
            }
            Request::Open {
                owner,
                intent,
                manifest,
                bootstrap,
            } => {
                self.prune_gate_jobs();
                if self.gate_jobs.rows.len() >= SESSION_CAP {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope: gate session capacity",
                        self.revision,
                    ));
                }
                let mut checked = Err("authority_unavailable: gate owner".to_string());
                let at = wall_time(owner.now_ms());
                self.panes.with_pane_authority(
                    owner.team(),
                    owner.pane(),
                    owner.presented(),
                    &mut |team| {
                        if let Some(team) = team {
                            checked = gate::authorize_job(
                                &self.ledger,
                                team,
                                owner.pane(),
                                owner.actor().unwrap_or_default(),
                                (*intent).clone(),
                                &manifest,
                                (*bootstrap).clone(),
                                at,
                            );
                        }
                    },
                );
                let scope = match checked {
                    Ok(scope) => scope,
                    Err(reason) => {
                        return Ok(answer(
                            false,
                            serde_json::Value::Null,
                            &reason,
                            self.revision,
                        ));
                    }
                };
                self.gate_jobs.next = self
                    .gate_jobs
                    .next
                    .checked_add(1)
                    .ok_or(RuntimeError::AuthorityRejected)?;
                let job = format!("gj-{}-{}", self.gate_jobs.epoch, self.gate_jobs.next);
                let expires_ms = scope.expires_ms;
                self.gate_jobs.rows.insert(
                    job.clone(),
                    Session {
                        namespace: job.clone(),
                        deadline: std::time::Instant::now()
                            + std::time::Duration::from_millis((expires_ms - at) as u64),
                        owner: *owner,
                        scope,
                        bound: false,
                        stage: Stage::Initial,
                        registration: None,
                        lease: None,
                        register_retry: None,
                    },
                );
                Ok(answer(
                    true,
                    serde_json::json!({"job":job,"expiresMs":expires_ms}),
                    "",
                    self.revision,
                ))
            }
            Request::Close { job } => {
                self.gate_jobs.rows.remove(&job);
                Ok(answer(true, serde_json::Value::Null, "", self.revision))
            }
            Request::Alive { job, now_ms } => {
                let checked = self
                    .gate_jobs
                    .rows
                    .get(&job)
                    .ok_or_else(|| "out_of_scope: closed gate job".to_string())
                    .and_then(|session| self.gate_authorized(session, wall_time(now_ms)));
                if checked.is_err() {
                    self.gate_jobs.rows.remove(&job);
                }
                Ok(answer(
                    checked.is_ok(),
                    serde_json::Value::Null,
                    checked.err().as_deref().unwrap_or_default(),
                    self.revision,
                ))
            }
            Request::Bind {
                job,
                capability,
                now_ms,
            } => {
                let Some(mut session) = self.gate_jobs.rows.remove(&job) else {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope: unknown gate job",
                        self.revision,
                    ));
                };
                let mut authorized = false;
                self.panes.with_pane_authority(
                    session.owner.team(),
                    session.owner.pane(),
                    &capability,
                    &mut |team| {
                        authorized = team.is_some();
                    },
                );
                if session.bound || !authorized {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope: gate connection",
                        self.revision,
                    ));
                }
                if let Err(reason) = self.gate_authorized(&session, wall_time(now_ms)) {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        &reason,
                        self.revision,
                    ));
                }
                session.bound = true;
                let binding = serde_json::json!({"policy":session.scope.policy,"producer":session.scope.producer});
                self.gate_jobs.rows.insert(job, session);
                Ok(answer(true, binding, "", self.revision))
            }
            Request::Command { job, body, now_ms } => {
                let Some(mut session) = self.gate_jobs.rows.remove(&job) else {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope: closed gate job",
                        self.revision,
                    ));
                };
                let at = wall_time(now_ms);
                if !session.bound {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        "out_of_scope: unbound gate job",
                        self.revision,
                    ));
                }
                if let Err(reason) = self.gate_authorized(&session, at) {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        &reason,
                        self.revision,
                    ));
                }
                let call = match gate_argv(&session, &body) {
                    Ok(value) => value,
                    Err(reason) => {
                        return Ok(answer(
                            false,
                            serde_json::Value::Null,
                            &reason,
                            self.revision,
                        ));
                    }
                };
                let mut command = session.owner.clone();
                command.argv = call.argv;
                command.now_ms = at;
                command.gate_scope = Some(Box::new(session.scope.clone()));
                command.gate_context = Some(
                    gate::JobContext::from_session(&session.namespace, &session.scope)
                        .map_err(|_| RuntimeError::AuthorityRejected)?,
                );
                let reply = match self.carry_out(command) {
                    Err(RuntimeError::GateJobRefused(reason)) => {
                        return Ok(answer(
                            false,
                            serde_json::Value::Null,
                            &reason,
                            self.revision,
                        ));
                    }
                    other => other?,
                };
                let RuntimeReply::Planned { decided, .. } = &reply else {
                    return Err(RuntimeError::AuthorityRejected);
                };
                if decided.reply.exit_code != 0 {
                    return Ok(answer(
                        false,
                        serde_json::Value::Null,
                        &decided.reply.stderr,
                        self.revision,
                    ));
                }
                let value: serde_json::Value = serde_json::from_str(&decided.reply.stdout)
                    .map_err(|_| RuntimeError::AuthorityRejected)?;
                if call.registering {
                    session.registration = value["registration"].as_str().map(str::to_string);
                    session.lease = value["lease"].as_str().map(str::to_string);
                    if session.registration.is_none() || session.lease.is_none() {
                        return Err(RuntimeError::AuthorityRejected);
                    }
                    session.register_retry = Some(call.retry);
                    session.stage = Stage::Registered;
                }
                if call.starting {
                    session.stage = Stage::Started;
                }
                if !call.terminal {
                    self.gate_jobs.rows.insert(job, session);
                }
                Ok(answer(true, value, "", self.revision))
            }
        }
    }
}

struct GateCommand {
    argv: Vec<String>,
    terminal: bool,
    registering: bool,
    starting: bool,
    retry: String,
}

fn gate_argv(session: &Session, call: &serde_json::Value) -> Result<GateCommand, String> {
    let object = call.as_object().ok_or("invalid_request: gate command")?;
    if object.keys().any(|key| {
        !["verb", "body", "retryRequest", "start", "cancel", "lease"].contains(&key.as_str())
    }) {
        return Err("unknown_field: gate command".into());
    }
    if serde_json::to_vec(call)
        .map_err(|_| "invalid_request: gate command")?
        .len()
        > FRAME_CAP
    {
        return Err("invalid_request: gate command size".into());
    }
    let verb = call["verb"].as_str().ok_or("invalid_request: gate verb")?;
    let retry = call["retryRequest"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or("invalid_request: gate retry")?;
    let body = call.get("body").ok_or("invalid_request: gate body")?;
    let scope = &session.scope;
    let mut extra = Vec::new();
    let mut terminal = false;
    let mut registering = false;
    let mut starting = false;
    match (verb, &scope.intent.mode) {
        (gate::REGISTER, JobMode::Run) => {
            if session.stage != Stage::Initial
                && !(session.stage == Stage::Registered
                    && session.register_retry.as_deref() == Some(retry))
            {
                return Err("out_of_scope: registration already issued".into());
            }
            let inputs: gate::Inputs = parsed(body)?;
            gate::validate_job_inputs(scope, &inputs)?;
            registering = true;
        }
        (gate::RECORD, JobMode::Run) => {
            if let Some(id) = call.get("start").or_else(|| call.get("cancel")) {
                if call.get("start").is_some() && call.get("cancel").is_some() {
                    return Err("out_of_scope: gate record mode".into());
                }
                if id.as_str() != session.registration.as_deref()
                    || call["lease"].as_str() != session.lease.as_deref()
                {
                    return Err("out_of_scope: another gate registration or lease".into());
                }
                if call.get("start").is_some() {
                    if session.stage != Stage::Registered {
                        return Err("out_of_scope: gate start already issued".into());
                    }
                    let inputs: gate::Inputs = parsed(body)?;
                    gate::validate_job_inputs(scope, &inputs)?;
                    starting = true;
                } else {
                    terminal = true;
                }
                extra.extend([
                    if starting { "--start" } else { "--cancel" }.into(),
                    id.as_str()
                        .ok_or("invalid_request: gate registration")?
                        .into(),
                    "--lease".into(),
                    session.lease.clone().ok_or("out_of_scope: gate lease")?,
                ]);
            } else {
                if call.get("lease").is_some() {
                    return Err("unknown_field: completion lease flag".into());
                }
                if session.stage != Stage::Started {
                    return Err("out_of_scope: gate completion without start".into());
                }
                let metadata: gate::Metadata = parsed(body)?;
                if Some(metadata.registration.as_str()) != session.registration.as_deref()
                    || Some(metadata.lease.as_str()) != session.lease.as_deref()
                    || metadata.policy_pin != scope.intent.pin
                {
                    return Err("out_of_scope: another gate completion".into());
                }
                gate::validate_job_inputs(scope, &metadata.inputs)?;
                terminal = true;
            }
        }
        (gate::REUSE, JobMode::Verify) => {
            let request: gate::ReuseRequest = parsed(body)?;
            if Some(request.registration.as_str()) != scope.intent.registration.as_deref() {
                return Err("out_of_scope: another gate receipt".into());
            }
            gate::validate_job_inputs(scope, &request.inputs)?;
            terminal = true;
        }
        _ => return Err("out_of_scope: gate session verb".into()),
    }
    if verb != gate::RECORD
        && ["start", "cancel", "lease"]
            .iter()
            .any(|key| call.get(key).is_some())
    {
        return Err("unknown_field: gate command flags".into());
    }
    let mut argv = vec![
        verb.into(),
        "--run".into(),
        scope.intent.run.clone(),
        "--task".into(),
        scope.intent.task.clone(),
        "--attempt".into(),
        scope.intent.attempt.clone(),
        "--body".into(),
        body.to_string(),
        "--retry-request".into(),
        format!(
            "gate-job-{}",
            zerocode_core::user_preferences::content_key(
                format!("{}:{retry}", session.namespace).as_bytes()
            )
        ),
    ];
    argv.extend(extra);
    Ok(GateCommand {
        argv,
        terminal,
        registering,
        starting,
        retry: retry.into(),
    })
}
