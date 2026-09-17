mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::server;
use russh::{Channel, ChannelId, Pty, Sig};
use tokio::time::{sleep, timeout};
use zerocode_core::{HostError, PtyCwd, PtySpec, RemotePath};
use zerocode_lane::{LaneSupervisor, PtyHandle, PtySpawner, PtyTransport, PtyTransportError};
use zerocode_pty::ZoBinary;
use zerocode_ssh::{
    PTY_OUTPUT_BYTE_BUDGET, SshConnection, SshConnector, SshPassword, SshPtySpawner,
};

use support::{ACCEPTED_PASSWORD, Fixture, USER};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const NORMAL_EXIT_STATUS: u32 = 7;
const KILLED_EXIT_STATUS: u32 = 137;

#[derive(Default)]
struct Observed {
    pty: Mutex<Option<(String, u32, u32)>>,
    environment: Mutex<Vec<(String, String)>>,
    command: Mutex<Option<Vec<u8>>>,
    input: Mutex<Vec<u8>>,
    resizes: Mutex<Vec<(u32, u32)>>,
    killed: AtomicBool,
    finished: AtomicBool,
}

struct PtyServer {
    observed: Arc<Observed>,
    reject_environment: bool,
    reply_to_pty: bool,
    initial_output: Vec<u8>,
}

struct StalledChannelOpenServer {
    opened: Arc<AtomicBool>,
    pending_reply: Option<server::ChannelOpenHandle>,
}

impl server::Handler for StalledChannelOpenServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if user == USER && password == ACCEPTED_PASSWORD {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.opened.store(true, Ordering::Release);
        self.pending_reply = Some(reply);
        Ok(())
    }
}

impl server::Handler for PtyServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if user == USER && password == ACCEPTED_PASSWORD {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        *self.observed.pty.lock().expect("pty observation lock") =
            Some((term.to_owned(), col_width, row_height));
        if self.reply_to_pty {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        *self
            .observed
            .command
            .lock()
            .expect("command observation lock") = Some(command.to_vec());
        session.channel_success(channel)?;
        session.data(channel, self.initial_output.clone())?;
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .environment
            .lock()
            .expect("environment observation lock")
            .push((variable_name.to_owned(), variable_value.to_owned()));
        if self.reject_environment {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let should_finish = {
            let mut input = self.observed.input.lock().expect("input observation lock");
            input.extend_from_slice(data);
            input
                .windows(b"finish\n".len())
                .any(|window| window == b"finish\n")
        };

        if should_finish && !self.observed.finished.swap(true, Ordering::AcqRel) {
            session.data(channel, b"done".to_vec())?;
            session.exit_status_request(channel, NORMAL_EXIT_STATUS)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .resizes
            .lock()
            .expect("resize observation lock")
            .push((col_width, row_height));
        Ok(())
    }

    async fn signal(
        &mut self,
        channel: ChannelId,
        signal: Sig,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if matches!(signal, Sig::KILL) {
            self.observed.killed.store(true, Ordering::Release);
        }
        session.exit_status_request(channel, KILLED_EXIT_STATUS)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

async fn fixture(reject_environment: bool) -> (Fixture, Arc<Observed>) {
    fixture_with(reject_environment, true, b"ready\x1b[c".to_vec()).await
}

async fn fixture_with(
    reject_environment: bool,
    reply_to_pty: bool,
    initial_output: Vec<u8>,
) -> (Fixture, Arc<Observed>) {
    let observed = Arc::new(Observed::default());
    let fixture = Fixture::start(PtyServer {
        observed: Arc::clone(&observed),
        reject_environment,
        reply_to_pty,
        initial_output,
    })
    .await;
    (fixture, observed)
}

async fn open_pty(fixture: &Fixture, spec: &PtySpec) -> PtyHandle {
    let connection = connect(fixture).await;
    timeout(TEST_TIMEOUT, connection.open_pty(spec))
        .await
        .expect("PTY open timed out")
        .expect("open fixture PTY")
}

async fn connect(fixture: &Fixture) -> SshConnection {
    let host = fixture.password_host(&fixture.public_key);
    SshConnector::default()
        .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())))
        .await
        .expect("connect fixture SSH server")
}

fn spec() -> PtySpec {
    PtySpec::remote(
        "/usr/bin/tool",
        &["arg; $(not-injected)".to_owned(), "a'b".to_owned()],
        RemotePath::parse("/srv/work dir").expect("valid remote path"),
        &[
            ("REMOVE_ME".to_owned(), String::new()),
            ("KEEP".to_owned(), "x'y".to_owned()),
        ],
        12,
        40,
    )
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    timeout(TEST_TIMEOUT, async {
        loop {
            if predicate() {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture observation timed out");
}

async fn pump_until_ended(pty: &mut PtyHandle) {
    timeout(TEST_TIMEOUT, async {
        loop {
            if pty.pump().ended {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("PTY did not end");
}

#[tokio::test]
async fn remote_pty_moves_command_io_resize_replies_and_exit_off_registry_path() {
    let (fixture, observed) = fixture(false).await;
    let mut pty = open_pty(&fixture, &spec()).await;

    wait_until(|| pty.pump().bytes > 0).await;
    wait_until(|| {
        observed
            .input
            .lock()
            .expect("input observation lock")
            .windows(b"\x1b[?1;2c".len())
            .any(|window| window == b"\x1b[?1;2c")
    })
    .await;

    pty.resize(20, 80).expect("queue remote resize");
    pty.write_input(b"finish\n").expect("queue remote input");
    pump_until_ended(&mut pty).await;

    assert_eq!(
        *observed.pty.lock().expect("pty observation lock"),
        Some(("xterm-256color".to_owned(), 40, 12))
    );
    assert_eq!(
        observed
            .resizes
            .lock()
            .expect("resize observation lock")
            .as_slice(),
        &[(80, 20)]
    );
    assert_eq!(
        observed
            .environment
            .lock()
            .expect("environment observation lock")
            .as_slice(),
        &[("KEEP".to_owned(), "x'y".to_owned())]
    );
    assert_eq!(
        String::from_utf8(
            observed
                .command
                .lock()
                .expect("command observation lock")
                .clone()
                .expect("command observed")
        )
        .expect("UTF-8 command"),
        "cd '/srv/work dir' && unset REMOVE_ME && exec '/usr/bin/tool' \
         'arg; $(not-injected)' 'a'\"'\"'b'"
    );
    assert_eq!(pty.try_wait().expect("read remote exit"), Some(7));
    assert_eq!(
        (pty.terminal().grid().rows(), pty.terminal().grid().cols()),
        (20, 80)
    );
    assert_eq!(pty.terminal().grid().visible_text(), "readydone");
}

#[tokio::test]
async fn kill_sends_only_the_remote_channel_kill_and_returns_stable_exit() {
    let (fixture, observed) = fixture(false).await;
    let mut pty = open_pty(&fixture, &spec()).await;

    pty.kill().expect("queue remote kill");
    wait_until(|| observed.killed.load(Ordering::Acquire)).await;
    pump_until_ended(&mut pty).await;

    assert_eq!(
        pty.try_wait().expect("read killed exit"),
        Some(KILLED_EXIT_STATUS)
    );
}

#[tokio::test]
async fn dropping_the_handle_terminates_its_detached_worker_and_channel() {
    let (fixture, observed) = fixture(false).await;
    let pty = open_pty(&fixture, &spec()).await;

    drop(pty);

    wait_until(|| observed.killed.load(Ordering::Acquire)).await;
}

#[tokio::test]
async fn rejected_remote_environment_fails_closed_before_exec() {
    let (fixture, observed) = fixture(true).await;
    let connection = connect(&fixture).await;

    let result = timeout(TEST_TIMEOUT, connection.open_pty(&spec()))
        .await
        .expect("PTY open timed out");

    assert!(matches!(
        result,
        Err(zerocode_ssh::ConnectError::RemoteEnvironmentRejected)
    ));
    assert_eq!(
        observed
            .environment
            .lock()
            .expect("environment observation lock")
            .as_slice(),
        &[("KEEP".to_owned(), "x'y".to_owned())]
    );
    assert!(
        observed
            .command
            .lock()
            .expect("command observation lock")
            .is_none(),
        "the server received exec after rejecting an environment value"
    );
}

#[tokio::test]
async fn a_stalled_pty_reply_hits_the_setup_deadline() {
    let (fixture, observed) = fixture_with(false, false, Vec::new()).await;
    let connection = connect(&fixture).await;
    let opening = tokio::spawn(async move { connection.open_pty(&spec()).await });
    wait_until(|| observed.pty.lock().expect("pty observation lock").is_some()).await;
    tokio::time::pause();

    let result = opening.await.expect("PTY setup task");

    assert!(matches!(result, Err(zerocode_ssh::ConnectError::Timeout)));
    assert!(
        observed
            .command
            .lock()
            .expect("command observation lock")
            .is_none(),
        "exec crossed a timed-out PTY request"
    );
}

#[tokio::test]
async fn a_stalled_channel_open_hits_the_setup_deadline() {
    let opened = Arc::new(AtomicBool::new(false));
    let fixture = Fixture::start(StalledChannelOpenServer {
        opened: Arc::clone(&opened),
        pending_reply: None,
    })
    .await;
    let connection = connect(&fixture).await;
    let opening = tokio::spawn(async move { connection.open_pty(&spec()).await });
    wait_until(|| opened.load(Ordering::Acquire)).await;
    tokio::time::pause();

    let result = opening.await.expect("channel setup task");

    assert!(matches!(result, Err(zerocode_ssh::ConnectError::Timeout)));
    assert!(opened.load(Ordering::Acquire));
}

#[tokio::test]
async fn terminal_reply_and_kill_preempt_output_waiting_on_the_byte_budget() {
    let mut flood = b"\x1b[c".to_vec();
    flood.resize(PTY_OUTPUT_BYTE_BUDGET * 2, b'x');
    let (fixture, observed) = fixture_with(false, true, flood).await;
    let mut pty = open_pty(&fixture, &spec()).await;

    sleep(Duration::from_millis(25)).await;
    let pumped = pty.pump();
    assert!(pumped.bytes > 0);
    assert!(pumped.bytes <= PTY_OUTPUT_BYTE_BUDGET);
    wait_until(|| {
        observed
            .input
            .lock()
            .expect("input observation lock")
            .windows(b"\x1b[?1;2c".len())
            .any(|window| window == b"\x1b[?1;2c")
    })
    .await;

    pty.kill().expect("queue kill behind blocked output");
    wait_until(|| observed.killed.load(Ordering::Acquire)).await;
    pump_until_ended(&mut pty).await;
}

#[tokio::test]
async fn dropping_a_transport_releases_a_worker_blocked_on_output_budget() {
    let flood = vec![b'x'; PTY_OUTPUT_BYTE_BUDGET * 2];
    let (fixture, observed) = fixture_with(false, true, flood).await;
    let pty = open_pty(&fixture, &spec()).await;

    sleep(Duration::from_millis(25)).await;
    drop(pty);

    wait_until(|| observed.killed.load(Ordering::Acquire)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_shot_spawner_composes_with_the_lane_supervisor_inside_tokio() {
    const SERVE_TOKEN: &str = "fixture-serve-token-not-for-exec";

    let (fixture, observed) = fixture(false).await;
    let connection = connect(&fixture).await;
    let supervisor = LaneSupervisor::new(
        ZoBinary {
            path: "/opt/zo".into(),
        },
        "127.0.0.1:43127",
        Some(SERVE_TOKEN.to_owned()),
    )
    .expect("fixture lane supervisor");
    let mut pty = supervisor
        .spawn_attach(
            Box::new(SshPtySpawner::new(
                connection,
                tokio::runtime::Handle::current(),
            )),
            Some("fixture-session"),
            Some(PtyCwd::Remote(
                RemotePath::parse("/srv/work dir").expect("valid remote path"),
            )),
            &[],
            12,
            40,
        )
        .expect("spawn SSH lane through supervisor");

    pty.write_input(b"finish\n").expect("queue remote input");
    pump_until_ended(&mut pty).await;

    assert_eq!(
        observed
            .environment
            .lock()
            .expect("environment observation lock")
            .as_slice(),
        &[("ZO_SERVE_TOKEN".to_owned(), SERVE_TOKEN.to_owned())]
    );
    let command = observed
        .command
        .lock()
        .expect("command observation lock")
        .clone()
        .expect("command observed");
    assert!(
        !command
            .windows(SERVE_TOKEN.len())
            .any(|window| window == SERVE_TOKEN.as_bytes()),
        "the lane token entered SSH exec bytes"
    );
}

#[test]
fn one_shot_spawner_can_open_from_outside_tokio() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("fixture runtime");
    let (fixture, connection) = runtime.block_on(async {
        let (fixture, _) = fixture(false).await;
        let connection = connect(&fixture).await;
        (fixture, connection)
    });

    let mut pty = Box::new(SshPtySpawner::new(connection, runtime.handle().clone()))
        .spawn(&spec())
        .expect("spawn outside Tokio");
    pty.kill().expect("queue remote kill");
    runtime.block_on(pump_until_ended(&mut pty));

    drop(pty);
    drop(fixture);
}

#[test]
fn current_thread_runtime_is_refused_without_panicking_or_opening_a_channel() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("fixture runtime");
    runtime.block_on(async {
        let (fixture, observed) = fixture(false).await;
        let connection = connect(&fixture).await;

        let result = Box::new(SshPtySpawner::new(
            connection,
            tokio::runtime::Handle::current(),
        ))
        .spawn(&spec());

        assert!(matches!(
            result,
            Err(PtyTransportError::Host(HostError::Transport))
        ));
        assert!(
            observed.pty.lock().expect("pty observation lock").is_none(),
            "the unsupported runtime reached PTY setup"
        );
    });
}
