//! The Android mirror the original streams from.
//!
//! `screenrecord` is Android's own recorder, and on a BUSY screen it keeps up.
//! Measured against a booted emulator here, five seconds of scrolling: 2,267
//! KiB in 255 chunks, median gap 17ms. That number is worth stating plainly,
//! because the reason to leave it is NOT throughput.
//!
//! It is everywhere else that the two differ. On an idle screen the recorder
//! hands over one 57 KiB lump and then says nothing at all for a minute, while
//! scrcpy keeps a steady ten packets a second — so the first motion after a
//! pause waits on an encoder that had stopped talking. It also stops dead
//! every three minutes, because `--time-limit` is a flag it must be given, and
//! the mirror blinks while a new encoder stands up. And it is a byte stream
//! with no marks in it: no packet boundary, no "this one is a key frame", no
//! "these are the parameters" — all of which scrcpy states outright.
//!
//! Under that same scrolling, scrcpy produced 197 packets in five seconds
//! (39/s, median gap 21ms, longest 161ms), and its first packet arrives 0.174s
//! after the socket opens (measured through this client, not a probe).
//!
//! The payload of those packets is Annex-B H.264 — the same bytes
//! `screenrecord` writes, measured: the first packet begins `00 00 00 01 67`
//! (SPS) and the second `00 00 00 01 65` (IDR). That is the whole reason this
//! can be added without the window learning anything: only the producer
//! changes.
//!
//! What this file owns is the session — the jar, the tunnel, the process and
//! the socket. The framing lives next door in [`crate::scrcpy_video`], which
//! has no I/O and can therefore be tested without a phone.

use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};

/// The server version this client's protocol is written against.
///
/// The original pinned 2.4 (`SCRCPY_SERVER_VERSION = '2.4'`); that server dies on
/// Android 15 with `ClassNotFoundException: com.genymobile.scrcpy.Server` before it
/// ever listens (measured 2026-09-21 on an API 35 emulator: refused every time, so the
/// pane fell to the recorder road and a screenshot took 28 s under load). 3.3.4 keeps
/// the same framing — device name, codec meta, packet headers — and answered its
/// readiness byte in 945 ms on the same emulator. The number is not decoration: the server refuses a client whose version string
/// does not match its own, which is what keeps a silently upgraded helper from
/// speaking a framing this side does not know.
pub const SERVER_VERSION: &str = "3.3.4";

/// Where the release the version names actually lives.
const SERVER_URL: &str =
    "https://github.com/Genymobile/scrcpy/releases/download/v3.3.4/scrcpy-server-v3.3.4";

/// What that release is, byte for byte.
///
/// Taken from the vendor's own published `SHA256SUMS.txt` for v3.3.4 and
/// checked against a fresh download here — they agree. The original checks
/// only that the file it fetched is at least ten kilobytes; this is the same
/// fetch with the vendor's own answer to "is this the file", which matters
/// more than usual for an artifact this window pushes onto a device and runs.
const SERVER_SHA256: &str = "8588238c9a5a00aa542906b6ec7e6d5541d9ffb9b5d0f6e1bc0e365e2303079e";

/// Where the server is pushed to on the device. The original's path, and
/// scrcpy's own default.
const DEVICE_JAR: &str = "/data/local/tmp/scrcpy-server.jar";

/// How long to keep knocking on the tunnel before giving up on a session.
///
/// The server has to start a virtual machine, open a display and bind its
/// socket before anything will answer. Measured here at well under a second;
/// this is the generous end of that, and reaching it means falling back rather
/// than failing.
const CONNECT_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// How long one knock waits for the readiness marker before knocking again.
const KNOCK_PATIENCE: std::time::Duration = std::time::Duration::from_millis(200);

/// How long one read waits before coming back empty-handed.
///
/// Not a liveness rule — it is how quickly a closed pane is noticed. The pump
/// only looks at its "still wanted?" flag between reads, so a read that could
/// block forever is a mirror that keeps running after the pane holding it is
/// gone. A quarter of a second is under a person's notice and far longer than
/// the gap between packets on an idle screen (ten a second, measured).
const STREAM_PATIENCE: std::time::Duration = std::time::Duration::from_millis(250);

/// A live mirror: the server on the device, the tunnel to it, and the socket
/// the pictures arrive on.
///
/// Dropping it takes all three down, in that order, because a tunnel left
/// behind survives the window that opened it and a server left behind holds
/// the device's display.
pub struct Session {
    server: std::process::Child,
    video: TcpStream,
    adb: PathBuf,
    serial: String,
    port: u16,
}

impl Session {
    /// Read whatever has arrived onto the end of `held`.
    ///
    /// # Errors
    ///
    /// Whatever the socket says. A read of zero is the server having gone,
    /// which is reported as an unexpected end rather than as a quiet success.
    pub fn read_into(&mut self, held: &mut Vec<u8>) -> std::io::Result<usize> {
        let mut chunk = [0u8; 64 * 1024];
        let read = self.video.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the scrcpy server closed the stream",
            ));
        }
        held.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    /// Stop waiting for pictures that are not coming.
    ///
    /// # Errors
    ///
    /// Whatever the socket says about the deadline it was given.
    pub fn set_patience(&self, patience: std::time::Duration) -> std::io::Result<()> {
        self.video.set_read_timeout(Some(patience))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = adb(&self.adb, &self.serial)
            .args(["forward", "--remove", &format!("tcp:{}", self.port)])
            .status();
    }
}

fn adb(adb: &Path, serial: &str) -> std::process::Command {
    let mut command = crate::proc::quiet_command(adb);
    command.args(["-s", serial]);
    command
}

/// The cached server jar, fetched once if this machine has never had it.
///
/// Answers `None` rather than an error for every way it can fail — no network,
/// a refused download, a file that is not the file we pinned — because the
/// caller's answer to all of them is the same: take the other road. The
/// refusal is worth a line in the black box, not a dialog.
pub async fn server_jar(cache_root: &Path) -> Option<PathBuf> {
    let path = cache_root
        .join("scrcpy")
        .join(format!("scrcpy-server-v{SERVER_VERSION}.jar"));
    if is_the_pinned_server(&path) {
        return Some(path);
    }
    let bytes = reqwest::get(SERVER_URL)
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if sha256_of(&bytes) != SERVER_SHA256 {
        return None;
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}

/// Is the file on disk the exact release this client was written against?
///
/// Hashed on every start rather than measured once: it costs a fraction of a
/// millisecond for seventy kilobytes, and it is the difference between
/// noticing a half-written download and pushing one onto a device.
fn is_the_pinned_server(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| sha256_of(&bytes) == SERVER_SHA256)
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The session id the server and the tunnel agree on.
///
/// scrcpy parses it as a SIGNED 32-bit hex integer, so the top bit is cleared
/// and the answer is padded to the eight digits the server's own `%08x`
/// produces — the original's rule, for the original's reason.
fn session_id() -> Option<String> {
    let token = crate::hooks::random_token()?;
    let number = u32::from_str_radix(token.get(..8)?, 16).ok()?;
    Some(format!("{:08x}", number & 0x7fff_ffff))
}

/// Start a mirror on `serial`.
///
/// # Errors
///
/// A sentence naming the step that refused. Every one of them is a reason to
/// take the other road rather than to stop mirroring.
pub fn start(
    adb_path: &Path,
    serial: &str,
    jar: &Path,
    max_size: u32,
    bit_rate: &str,
) -> Result<Session, String> {
    // Pushed on EVERY session, not once: `cleanup=true` has the server delete
    // this file on its way out. Measured here — the second session on a device
    // that had one before it died with "Aborted" and nothing else, because the
    // jar the first session pushed was gone.
    let pushed = adb(adb_path, serial)
        .args(["push"])
        .arg(jar)
        .arg(DEVICE_JAR)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if !pushed.success() {
        return Err("scrcpy 서버를 기기에 보내지 못했습니다".to_string());
    }
    let scid = session_id().ok_or("스트림 id를 만들 수 없습니다")?;
    // `tcp:0` asks adb to choose, and it answers with the port it chose —
    // asking for a fixed one is how two windows on one machine take each
    // other's mirror.
    let forwarded = adb(adb_path, serial)
        .args(["forward", "tcp:0", &format!("localabstract:scrcpy_{scid}")])
        .output()
        .map_err(|error| error.to_string())?;
    let port: u16 = String::from_utf8_lossy(&forwarded.stdout)
        .trim()
        .parse()
        .map_err(|_| "adb가 터널 포트를 말해 주지 않았습니다".to_string())?;
    let mut server = adb(adb_path, serial)
        .args([
            "shell",
            &format!("CLASSPATH={DEVICE_JAR}"),
            "app_process",
            "/",
            "com.genymobile.scrcpy.Server",
            SERVER_VERSION,
            &format!("scid={scid}"),
            "log_level=info",
            "tunnel_forward=true",
            "audio=false",
            // The original asks for a control socket; this window does not,
            // because its taps and keystrokes already go out through
            // `adb shell input` — a road that is measured, injected and
            // gated on a live serial. A second control channel would be a
            // second answer to "who may touch this device".
            "control=false",
            "cleanup=true",
            "clipboard_autosync=false",
            "video_codec=h264",
            &format!("max_size={max_size}"),
            &format!("video_bit_rate={bit_rate}"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let Some(video) = knock(port) else {
        // The tunnel is ours and so is the server; neither may outlive a
        // session that never opened. `Drop` cannot do this one — there is no
        // session yet to drop.
        let _ = server.kill();
        let _ = server.wait();
        let _ = adb(adb_path, serial)
            .args(["forward", "--remove", &format!("tcp:{port}")])
            .status();
        return Err("scrcpy 서버가 응답하지 않았습니다".to_string());
    };
    let session = Session {
        server,
        video,
        adb: adb_path.to_path_buf(),
        serial: serial.to_string(),
        port,
    };
    // A mirror that has stopped speaking must not hold the pump forever: the
    // read comes back, the session is dropped, and the road below it is tried
    // on the next turn of the loop.
    session
        .set_patience(STREAM_PATIENCE)
        .map_err(|error| error.to_string())?;
    Ok(session)
}

/// Knock on the tunnel until the SERVER answers, not just the tunnel.
///
/// A forward accepts a connection whether or not anything is listening on the
/// device end — measured here: the first attempt connected instantly and then
/// closed with nothing on it, because the server had not bound its socket yet.
/// That is what scrcpy's one-byte readiness marker is for, and this waits to
/// see it before calling a session open. The byte is PEEKED rather than read,
/// so the handshake the caller parses is still whole.
fn knock(port: u16) -> Option<TcpStream> {
    let deadline = std::time::Instant::now() + CONNECT_PATIENCE;
    while std::time::Instant::now() < deadline {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port))
            && stream.set_read_timeout(Some(KNOCK_PATIENCE)).is_ok()
            && stream.peek(&mut [0u8; 1]).is_ok_and(|seen| seen == 1)
        {
            return Some(stream);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{SERVER_SHA256, SERVER_VERSION, sha256_of};

    /// The pinned hash is the shape a SHA-256 is, and the version is the one
    /// the URL beside it names — two constants that must agree with a third
    /// thing nobody can see from here, so at least say what they are.
    #[test]
    fn the_pinned_server_is_named_and_hashed() {
        assert_eq!(SERVER_VERSION, "3.3.4");
        assert_eq!(SERVER_SHA256.len(), 64);
        assert!(SERVER_SHA256.chars().all(|one| one.is_ascii_hexdigit()));
        assert!(super::SERVER_URL.ends_with(&format!("scrcpy-server-v{SERVER_VERSION}")));
    }

    /// A live mirror, against a real device.
    ///
    /// Ignored by default because it needs three things a build machine has
    /// no business assuming: a booted Android, an `adb` to reach it with, and
    /// the server jar already fetched. Given all three it is the only test
    /// that can say the handshake, the framing and the payload are what this
    /// client believes — everything else here is arithmetic.
    ///
    /// ```text
    /// ZEROCODE_LIVE_ADB=$ANDROID_HOME/platform-tools/adb \
    /// ZEROCODE_LIVE_SERIAL=emulator-5554 \
    /// ZEROCODE_LIVE_SCRCPY_JAR=/path/to/scrcpy-server-v3.3.4 \
    ///   cargo test -p zerocode-shell -- --ignored --nocapture a_live_mirror
    /// ```
    #[test]
    #[ignore = "needs a booted Android device and the server jar"]
    fn a_live_mirror_speaks_the_framing_this_client_believes() {
        use std::path::PathBuf;
        let (Ok(adb), Ok(serial), Ok(jar)) = (
            std::env::var("ZEROCODE_LIVE_ADB"),
            std::env::var("ZEROCODE_LIVE_SERIAL"),
            std::env::var("ZEROCODE_LIVE_SCRCPY_JAR"),
        ) else {
            println!("LIVE: no device named; nothing measured");
            return;
        };
        let mut session = super::start(
            &PathBuf::from(adb),
            &serial,
            &PathBuf::from(jar),
            1280,
            "8000000",
        )
        .expect("a mirror opens");
        // A read that comes back empty-handed is a screen nobody touched,
        // not a mirror that died — the client's deadline is deliberately
        // short so a closed pane is noticed quickly, and the pump answers a
        // quiet one by asking again. So does this, under a deadline long
        // enough that "nothing ever arrives" still fails. Measured: an idle
        // emulator can go past a second between packets.
        fn more(session: &mut super::Session, held: &mut Vec<u8>) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                match session.read_into(held) {
                    Ok(_) => return,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the mirror said nothing at all for ten seconds"
                        );
                    }
                    Err(error) => panic!("the socket speaks: {error}"),
                }
            }
        }
        let mut held = Vec::new();
        let opened = std::time::Instant::now();
        let name = loop {
            more(&mut session, &mut held);
            if let Some(name) = crate::scrcpy_video::take_handshake(&mut held) {
                break name;
            }
        };
        let meta = loop {
            if let Some(meta) = crate::scrcpy_video::take_codec_meta(&mut held) {
                break meta;
            }
            more(&mut session, &mut held);
        };
        println!(
            "LIVE: {name:?} {} {}x{}",
            meta.codec, meta.width, meta.height
        );
        assert_eq!(
            meta.codec, "h264",
            "the server sent a codec nobody asked for"
        );
        let mut packets = Vec::new();
        while packets.len() < 3 {
            more(&mut session, &mut held);
            packets.extend(crate::scrcpy_video::take_packets(&mut held).expect("framing holds"));
        }
        println!(
            "LIVE: first packet after {:?}, {} packets, first {} bytes",
            opened.elapsed(),
            packets.len(),
            packets[0].data.len()
        );
        // The opening packet is the parameter set, and it is Annex-B: a start
        // code followed by a NAL whose low five bits are 7 (SPS). The window's
        // decoder splits exactly this and nothing else.
        assert!(
            packets[0].config,
            "the stream opened without its parameters"
        );
        assert_eq!(
            &packets[0].data[..4],
            &[0, 0, 0, 1],
            "the payload is not Annex-B"
        );
        assert_eq!(packets[0].data[4] & 0x1f, 7, "the first NAL is not an SPS");
        assert!(
            packets.iter().any(|one| one.key_frame),
            "no key frame arrived, so nothing could ever be drawn"
        );
    }

    /// The hasher answers what `shasum -a 256` answers.
    #[test]
    fn the_hash_is_the_one_the_command_line_prints() {
        assert_eq!(
            sha256_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_of(b"scrcpy"),
            "9af98926f1141e463b4ae17b37d890e8d94adaed2fed78f1733bbd89e20f9ad5"
        );
    }
}
