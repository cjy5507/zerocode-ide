use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use russh::{Channel, ChannelMsg, Sig, client};
use tokio::sync::mpsc::{
    Receiver, Sender, UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use zerocode_core::{HostError, PtySpec};
use zerocode_lane::{PtyHandle, PtyTransport, PtyTransportError};
use zerocode_pty::{Pumped, Terminal};

use crate::ConnectError;
use crate::channel_reply::await_request_reply;
use crate::command::encode_remote_command;
use crate::connection::shutdown_handle;
use crate::deadline;
use crate::host_key::PinnedHostKeyHandler;

const NO_EXIT_STATUS: u64 = u64::MAX;
const UNKNOWN_REMOTE_EXIT_STATUS: u32 = 255;
const KILLED_EXIT_STATUS: u32 = 137;
/// Maximum decoded output bytes owned by the synchronous PTY pump queue —
/// the local lanes' bound, kept in one place rather than agreed upon twice.
pub use zerocode_pty::PTY_OUTPUT_BYTE_BUDGET;
pub(crate) const SSH_CHANNEL_PACKET_BYTES: u32 = 32 * 1024;
pub(crate) const OUTPUT_MESSAGE_CAPACITY: usize = 64;
const _: () = assert!(SSH_CHANNEL_PACKET_BYTES as usize <= PTY_OUTPUT_BYTE_BUDGET);

enum WorkerCommand {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Kill,
}

struct QueuedOutput {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

struct BoundedOutputSender {
    sender: Sender<QueuedOutput>,
    budget: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputSendError {
    Closed,
    Oversized,
}

impl BoundedOutputSender {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), OutputSendError> {
        if bytes.len() > PTY_OUTPUT_BYTE_BUDGET {
            return Err(OutputSendError::Oversized);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let permits = u32::try_from(bytes.len()).expect("output budget fits in u32");
        let permit = Arc::clone(&self.budget)
            .acquire_many_owned(permits)
            .await
            .map_err(|_| OutputSendError::Closed)?;
        self.sender
            .send(QueuedOutput {
                bytes,
                _permit: permit,
            })
            .await
            .map_err(|_| OutputSendError::Closed)
    }
}

fn output_channel() -> (BoundedOutputSender, Receiver<QueuedOutput>) {
    let (sender, receiver) = tokio::sync::mpsc::channel(OUTPUT_MESSAGE_CAPACITY);
    (
        BoundedOutputSender {
            sender,
            budget: Arc::new(Semaphore::new(PTY_OUTPUT_BYTE_BUDGET)),
        },
        receiver,
    )
}

struct WorkerState {
    ended: AtomicBool,
    failed: AtomicBool,
    exit_status: AtomicU64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerAction {
    Continue,
    Stop,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            ended: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            exit_status: AtomicU64::new(NO_EXIT_STATUS),
        }
    }

    fn exit_status(&self) -> Option<u32> {
        let status = self.exit_status.load(Ordering::Acquire);
        (status != NO_EXIT_STATUS).then_some(status as u32)
    }

    fn set_exit_status(&self, status: u32) {
        let _ = self.exit_status.compare_exchange(
            NO_EXIT_STATUS,
            u64::from(status),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn finish(&self) {
        if self.exit_status().is_none() && !self.failed.load(Ordering::Acquire) {
            self.set_exit_status(UNKNOWN_REMOTE_EXIT_STATUS);
        }
        self.ended.store(true, Ordering::Release);
    }

    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.ended.store(true, Ordering::Release);
    }
}

struct SshPtyTransport {
    commands: UnboundedSender<WorkerCommand>,
    output: Receiver<QueuedOutput>,
    terminal: Terminal,
    state: Arc<WorkerState>,
    kill_queued: bool,
}

impl SshPtyTransport {
    fn send(&self, command: WorkerCommand) -> Result<(), PtyTransportError> {
        self.commands.send(command).map_err(|_| transport_error())
    }

    fn ensure_running(&self) -> Result<(), PtyTransportError> {
        if self.state.ended.load(Ordering::Acquire) {
            Err(transport_error())
        } else {
            Ok(())
        }
    }
}

impl PtyTransport for SshPtyTransport {
    fn pump(&mut self) -> Pumped {
        let mut bytes = 0;
        let queued_at_start = self.output.len();
        for _ in 0..queued_at_start {
            match self.output.try_recv() {
                Ok(chunk) => {
                    bytes += chunk.bytes.len();
                    self.terminal.feed(&chunk.bytes);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state.ended.store(true, Ordering::Release);
                    break;
                }
            }
        }
        if self.output.is_closed() && self.output.is_empty() {
            self.state.ended.store(true, Ordering::Release);
        }

        let replies = self.terminal.grid_mut().take_replies();
        if !replies.is_empty()
            && !self.state.ended.load(Ordering::Acquire)
            && self.send(WorkerCommand::Input(replies)).is_err()
        {
            self.state.fail();
        }

        // A remote child's answer to a write is not timed here: its output
        // crosses a network first, so no chase inside one display frame
        // would catch it anyway.
        Pumped {
            bytes,
            ended: self.state.ended.load(Ordering::Acquire),
            answered: false,
            unanswered_since: None,
        }
    }

    fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
        self.ensure_running()?;
        self.send(WorkerCommand::Input(bytes.to_vec()))
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTransportError> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let grid = self.terminal.grid();
        if grid.rows() == rows as usize && grid.cols() == cols as usize {
            return Ok(());
        }
        self.ensure_running()?;
        self.send(WorkerCommand::Resize { rows, cols })?;
        self.terminal
            .grid_mut()
            .resize(rows as usize, cols as usize);
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
        if let Some(status) = self.state.exit_status() {
            return Ok(Some(status));
        }
        if self.state.failed.load(Ordering::Acquire) {
            return Err(transport_error());
        }
        Ok(None)
    }

    fn kill(&mut self) -> Result<(), PtyTransportError> {
        self.ensure_running()?;
        if !self.kill_queued {
            self.send(WorkerCommand::Kill)?;
            self.kill_queued = true;
        }
        Ok(())
    }
}

impl Drop for SshPtyTransport {
    fn drop(&mut self) {
        if !self.kill_queued && !self.state.ended.load(Ordering::Acquire) {
            let _ = self.commands.send(WorkerCommand::Kill);
        }
    }
}

enum SessionLease {
    Owned(client::Handle<PinnedHostKeyHandler>),
    Shared(Arc<crate::SshConnection>),
}

impl SessionLease {
    fn handle(&self) -> &client::Handle<PinnedHostKeyHandler> {
        match self {
            Self::Owned(handle) => handle,
            Self::Shared(connection) => &connection.handle,
        }
    }

    async fn finish(&mut self) {
        if let Self::Owned(handle) = self {
            let _ = shutdown_handle(handle).await;
        }
    }
}

pub(crate) async fn open(
    session: client::Handle<PinnedHostKeyHandler>,
    spec: &PtySpec,
) -> Result<PtyHandle, ConnectError> {
    open_leased(SessionLease::Owned(session), spec).await
}

pub(crate) async fn open_shared(
    session: Arc<crate::SshConnection>,
    spec: &PtySpec,
) -> Result<PtyHandle, ConnectError> {
    open_leased(SessionLease::Shared(session), spec).await
}

async fn open_leased(mut session: SessionLease, spec: &PtySpec) -> Result<PtyHandle, ConnectError> {
    let rows = spec.rows.max(1);
    let cols = spec.cols.max(1);
    let prepared = tokio::time::timeout(
        deadline::PTY_SETUP,
        prepare_channel(session.handle(), spec, rows, cols),
    )
    .await
    .unwrap_or(Err(ConnectError::Timeout));
    let channel = match prepared {
        Ok(channel) => channel,
        Err(error) => {
            session.finish().await;
            return Err(error);
        }
    };

    let (commands, command_receiver) = unbounded_channel();
    let (output_sender, output) = output_channel();
    let state = Arc::new(WorkerState::new());
    let transport = SshPtyTransport {
        commands,
        output,
        terminal: Terminal::new(rows as usize, cols as usize),
        state: Arc::clone(&state),
        kill_queued: false,
    };

    drop(tokio::spawn(run_worker(
        session,
        channel,
        command_receiver,
        output_sender,
        state,
    )));

    Ok(PtyHandle::new(transport))
}

async fn prepare_channel(
    session: &client::Handle<PinnedHostKeyHandler>,
    spec: &PtySpec,
    rows: u16,
    cols: u16,
) -> Result<Channel<client::Msg>, ConnectError> {
    let command = encode_remote_command(spec)?;
    let mut channel = tokio::time::timeout(deadline::CHANNEL_OPEN, session.channel_open_session())
        .await
        .map_err(|_| ConnectError::Timeout)??;

    tokio::time::timeout(deadline::CHANNEL_REQUEST, async {
        channel
            .request_pty(
                true,
                "xterm-256color",
                u32::from(cols),
                u32::from(rows),
                0,
                0,
                &[],
            )
            .await?;
        await_request_reply(&mut channel, ConnectError::PtyRequestRejected).await
    })
    .await
    .map_err(|_| ConnectError::Timeout)??;
    for (name, value) in command.environment {
        tokio::time::timeout(deadline::CHANNEL_REQUEST, async {
            channel.set_env(true, name, value).await?;
            await_request_reply(&mut channel, ConnectError::RemoteEnvironmentRejected).await
        })
        .await
        .map_err(|_| ConnectError::Timeout)??;
    }
    tokio::time::timeout(deadline::CHANNEL_REQUEST, async {
        channel.exec(true, command.shell.into_bytes()).await?;
        await_request_reply(&mut channel, ConnectError::PtyRequestRejected).await
    })
    .await
    .map_err(|_| ConnectError::Timeout)??;
    Ok(channel)
}

async fn run_worker(
    mut session: SessionLease,
    mut channel: Channel<client::Msg>,
    mut commands: UnboundedReceiver<WorkerCommand>,
    output: BoundedOutputSender,
    state: Arc<WorkerState>,
) {
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                if apply_worker_command(command, &channel, &state).await == WorkerAction::Stop {
                    break;
                }
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data })
                    | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if forward_output(
                            data.to_vec(),
                            &output,
                            &mut commands,
                            &channel,
                            &state,
                        )
                        .await
                            == WorkerAction::Stop
                        {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        state.set_exit_status(exit_status);
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        state.set_exit_status(signal_exit_status(&signal_name));
                    }
                    Some(ChannelMsg::Eof) => {
                        state.ended.store(true, Ordering::Release);
                    }
                    Some(ChannelMsg::Close) | None => break,
                    Some(ChannelMsg::Failure) => {
                        state.fail();
                        break;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    state.finish();
    let _ = channel.close().await;
    session.finish().await;
}

async fn forward_output(
    bytes: Vec<u8>,
    output: &BoundedOutputSender,
    commands: &mut UnboundedReceiver<WorkerCommand>,
    channel: &Channel<client::Msg>,
    state: &WorkerState,
) -> WorkerAction {
    let sending = output.send(bytes);
    tokio::pin!(sending);
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                if apply_worker_command(command, channel, state).await == WorkerAction::Stop {
                    return WorkerAction::Stop;
                }
            }
            result = &mut sending => {
                if result.is_err() {
                    return fail_channel(channel, state).await;
                }
                return WorkerAction::Continue;
            }
        }
    }
}

async fn apply_worker_command(
    command: Option<WorkerCommand>,
    channel: &Channel<client::Msg>,
    state: &WorkerState,
) -> WorkerAction {
    let result = match command {
        Some(WorkerCommand::Input(bytes)) => {
            tokio::time::timeout(deadline::CHANNEL_OPERATION, channel.data_bytes(bytes)).await
        }
        Some(WorkerCommand::Resize { rows, cols }) => {
            tokio::time::timeout(
                deadline::CHANNEL_OPERATION,
                channel.window_change(u32::from(cols), u32::from(rows), 0, 0),
            )
            .await
        }
        Some(WorkerCommand::Kill) | None => return terminate_channel(channel, state).await,
    };
    match result {
        Ok(Ok(())) => WorkerAction::Continue,
        Ok(Err(_)) | Err(_) => {
            state.fail();
            WorkerAction::Stop
        }
    }
}

async fn terminate_channel(channel: &Channel<client::Msg>, state: &WorkerState) -> WorkerAction {
    if signal_and_close(channel).await {
        state.set_exit_status(KILLED_EXIT_STATUS);
    } else {
        state.fail();
    }
    WorkerAction::Stop
}

async fn fail_channel(channel: &Channel<client::Msg>, state: &WorkerState) -> WorkerAction {
    state.fail();
    let _ = signal_and_close(channel).await;
    WorkerAction::Stop
}

async fn signal_and_close(channel: &Channel<client::Msg>) -> bool {
    matches!(
        tokio::time::timeout(deadline::CHANNEL_OPERATION, async {
            channel.signal(Sig::KILL).await?;
            channel.close().await
        })
        .await,
        Ok(Ok(()))
    )
}

fn signal_exit_status(signal: &Sig) -> u32 {
    128 + match signal {
        Sig::HUP => 1,
        Sig::INT => 2,
        Sig::QUIT => 3,
        Sig::ILL => 4,
        Sig::ABRT => 6,
        Sig::FPE => 8,
        Sig::KILL => 9,
        Sig::SEGV => 11,
        Sig::PIPE => 13,
        Sig::ALRM => 14,
        Sig::TERM => 15,
        Sig::USR1 => 10,
        Sig::Custom(_) => return UNKNOWN_REMOTE_EXIT_STATUS,
    }
}

fn transport_error() -> PtyTransportError {
    HostError::Transport.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn fast_output_stops_at_the_byte_budget_then_delivers_every_byte() {
        let (output, mut receiver) = output_channel();
        let budget = Arc::clone(&output.budget);
        let expected = vec![b'x'; PTY_OUTPUT_BYTE_BUDGET + 17];
        let producer = tokio::spawn(async move {
            output.send(vec![b'x'; PTY_OUTPUT_BYTE_BUDGET]).await?;
            output.send(vec![b'x'; 17]).await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if budget.available_permits() == 0 && receiver.len() == 1 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("producer never filled the byte budget");
        assert!(
            !producer.is_finished(),
            "oversized output bypassed the budget"
        );

        let first = receiver.recv().await.expect("first bounded output chunk");
        assert_eq!(first.bytes.len(), PTY_OUTPUT_BYTE_BUDGET);
        let mut delivered = first.bytes.clone();
        drop(first);

        let second = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("producer did not resume after pump")
            .expect("second bounded output chunk");
        delivered.extend_from_slice(&second.bytes);
        drop(second);
        assert_eq!(producer.await.expect("producer task"), Ok(()));
        assert_eq!(delivered, expected);
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET);
    }

    #[tokio::test]
    async fn an_oversized_protocol_message_fails_without_holding_budget() {
        let (output, _receiver) = output_channel();

        assert_eq!(
            output.send(vec![b'x'; PTY_OUTPUT_BYTE_BUDGET + 1]).await,
            Err(OutputSendError::Oversized)
        );
        assert_eq!(output.budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET);
    }

    #[tokio::test]
    async fn pump_and_transport_drop_return_every_output_permit() {
        let (output, receiver) = output_channel();
        let budget = Arc::clone(&output.budget);
        let (commands, _worker_commands) = unbounded_channel();
        let mut transport = SshPtyTransport {
            commands,
            output: receiver,
            terminal: Terminal::new(4, 8),
            state: Arc::new(WorkerState::new()),
            kill_queued: false,
        };

        output.send(vec![b'a'; 19]).await.expect("queue output");
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET - 19);
        assert_eq!(transport.pump().bytes, 19);
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET);

        output.send(vec![b'b'; 23]).await.expect("queue output");
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET - 23);
        drop(transport);
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_pump_returns_after_its_starting_snapshot_under_sustained_output() {
        let (output, receiver) = output_channel();
        let budget = Arc::clone(&output.budget);
        for _ in 0..OUTPUT_MESSAGE_CAPACITY {
            output.send(vec![b'x']).await.expect("prefill output");
        }
        let producer = tokio::spawn(async move {
            loop {
                if output.send(vec![b'y']).await.is_err() {
                    return;
                }
            }
        });
        let (commands, _worker_commands) = unbounded_channel();
        let mut transport = SshPtyTransport {
            commands,
            output: receiver,
            terminal: Terminal::new(4, 8),
            state: Arc::new(WorkerState::new()),
            kill_queued: false,
        };

        assert_eq!(transport.pump().bytes, OUTPUT_MESSAGE_CAPACITY);
        assert!(!producer.is_finished(), "producer did not sustain output");

        drop(transport);
        tokio::time::timeout(Duration::from_secs(1), producer)
            .await
            .expect("producer stayed blocked after transport drop")
            .expect("producer task");
        assert_eq!(budget.available_permits(), PTY_OUTPUT_BYTE_BUDGET);
    }
}
