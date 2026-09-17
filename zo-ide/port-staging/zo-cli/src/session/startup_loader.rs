use std::io::{self, IsTerminal, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];

/// Shows progress while the synchronous runtime is being assembled.
///
/// Runtime construction must stay on the calling thread because the runtime
/// owns non-`Send` resources such as MCP stdio transports. The loader therefore
/// animates independently and is scoped only around that blocking operation.
pub(crate) struct StartupLoader {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl StartupLoader {
    /// Starts the loader when both standard streams are interactive terminals.
    pub(crate) fn start() -> Self {
        let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
        Self::start_with_writer(interactive, io::stdout())
    }

    fn start_with_writer<W>(interactive: bool, writer: W) -> Self
    where
        W: Write + Send + 'static,
    {
        if !interactive {
            return Self::inactive();
        }

        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("zo-startup-loader".to_owned())
            .spawn(move || run(&worker_stop, writer))
            .ok();

        Self { stop, worker }
    }

    fn inactive() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }
}

impl Drop for StartupLoader {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };

        self.stop.store(true, Ordering::Release);
        worker.thread().unpark();
        let _ = worker.join();
    }
}

fn run<W>(stop: &AtomicBool, mut writer: W)
where
    W: Write,
{
    let mut frame = 0;

    while !stop.load(Ordering::Acquire) {
        if write_frame(&mut writer, FRAMES[frame]).is_err() {
            break;
        }
        frame = (frame + 1) % FRAMES.len();
        thread::park_timeout(FRAME_INTERVAL);
    }

    let _ = write!(writer, "\r\x1b[2K");
    let _ = writer.flush();
}

fn write_frame<W>(writer: &mut W, frame: &str) -> io::Result<()>
where
    W: Write,
{
    write!(writer, "\r\x1b[2K  {frame} Starting zo…")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::{FRAMES, StartupLoader};
    use std::io::{self, Write};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct RecordingWriter {
        output: Arc<Mutex<Vec<u8>>>,
        frame_seen: Arc<AtomicBool>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes
                .windows(b"Starting zo".len())
                .any(|window| window == b"Starting zo")
            {
                self.frame_seen.store(true, Ordering::Release);
            }
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn output_for_build(result: Result<(), &'static str>) -> (Result<(), &'static str>, Vec<u8>) {
        let output = Arc::new(Mutex::new(Vec::new()));
        let frame_seen = Arc::new(AtomicBool::new(false));
        let writer = RecordingWriter {
            output: Arc::clone(&output),
            frame_seen: Arc::clone(&frame_seen),
        };
        let loader = StartupLoader::start_with_writer(true, writer);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !frame_seen.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            frame_seen.load(Ordering::Acquire),
            "startup loader did not render a frame"
        );

        drop(loader);
        let bytes = Arc::try_unwrap(output)
            .expect("loader worker should release its output handle")
            .into_inner()
            .expect("recording writer mutex should not be poisoned");
        (result, bytes)
    }

    fn assert_started_and_cleared(output: &[u8]) {
        let rendered = String::from_utf8_lossy(output);
        assert!(rendered.contains("Starting zo…"), "missing startup frame: {rendered:?}");
        assert!(
            output.ends_with(b"\r\x1b[2K"),
            "loader did not clear its line: {output:?}"
        );
    }

    #[test]
    fn loader_renders_and_clears_on_success_and_build_error() {
        // CI has no real TTY; injecting the TTY decision and writer keeps the
        // actual worker lifecycle deterministic without weakening the branch.
        let (success, success_output) = output_for_build(Ok(()));
        assert!(success.is_ok());
        assert_started_and_cleared(&success_output);

        let (failure, failure_output) = output_for_build(Err("runtime build failed"));
        assert_eq!(failure, Err("runtime build failed"));
        assert_started_and_cleared(&failure_output);
    }

    #[test]
    fn noninteractive_loader_is_silent() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = RecordingWriter {
            output: Arc::clone(&output),
            frame_seen: Arc::new(AtomicBool::new(false)),
        };

        let loader = StartupLoader::start_with_writer(false, writer);
        drop(loader);

        assert!(output.lock().unwrap().is_empty());
    }

    #[test]
    fn spinner_frames_are_stable_and_ascii() {
        assert_eq!(FRAMES, ["|", "/", "-", "\\"]);
    }
}
