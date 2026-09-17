//! Exercise file operations against OpenSSH's actual SFTP implementation over
//! an authenticated SSH fixture. Only temporary directories are used.
#![cfg(unix)]
mod support;
use russh::{
    Channel, ChannelId, Pty,
    server::{self, Auth, ChannelOpenHandle, Msg, Session},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::{ACCEPTED_PASSWORD, Fixture, USER};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use zerocode_lane::PtyTransport;
use zerocode_ssh::{
    SshConnector, SshPassword,
    files::{self, Conflict, FileSystem, TransferControl},
};

struct FileServer {
    root: PathBuf,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    authentications: Arc<AtomicUsize>,
}
impl server::Handler for FileServer {
    type Error = russh::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.authentications.fetch_add(1, Ordering::Relaxed);
        Ok(if user == USER && password == ACCEPTED_PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(id)?;
            return Ok(());
        }
        let binary = std::env::var_os("SFTP_SERVER")
            .map(PathBuf::from)
            .or_else(|| {
                [
                    "/usr/libexec/sftp-server",
                    "/usr/lib/openssh/sftp-server",
                    "/usr/lib/ssh/sftp-server",
                ]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
            })
            .expect("OpenSSH sftp-server must be installed (or set SFTP_SERVER)");
        let mut child = tokio::process::Command::new(binary)
            .arg("-d")
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start OpenSSH SFTP fixture");
        let channel = self.channels.lock().await.remove(&id).expect("channel");
        session.channel_success(id)?;
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(channel.into_stream());
            let mut stdin = child.stdin.take().expect("stdin");
            let mut stdout = child.stdout.take().expect("stdout");
            tokio::select! {
                _ = async { let mut bytes = [0; 32768]; loop { let n = read.read(&mut bytes).await?; if n == 0 { break; } stdin.write_all(&bytes[..n]).await?; stdin.flush().await?; } Ok::<(), std::io::Error>(()) } => {},
                _ = async { let mut bytes = [0; 32768]; loop { let n = stdout.read(&mut bytes).await?; if n == 0 { break; } write.write_all(&bytes[..n]).await?; write.flush().await?; } Ok::<(), std::io::Error>(()) } => {},
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        });
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        _: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(id)?;
        session.exit_status_request(id, 0)?;
        session.close(id)?;
        Ok(())
    }
}

struct RemoteFixture {
    _server: Fixture,
    root: tempfile::TempDir,
    ssh: Arc<zerocode_ssh::SshConnection>,
    remote: FileSystem,
    auth: Arc<AtomicUsize>,
}
async fn fixture() -> RemoteFixture {
    let root = tempfile::tempdir().expect("remote root");
    let auth = Arc::new(AtomicUsize::new(0));
    let server = Fixture::start(FileServer {
        root: root.path().to_path_buf(),
        channels: Arc::default(),
        authentications: auth.clone(),
    })
    .await;
    let ssh = Arc::new(
        SshConnector::default()
            .connect(
                &server.password_host(&server.public_key),
                Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())),
            )
            .await
            .expect("SSH connect"),
    );
    let remote = FileSystem::Remote(Arc::new(ssh.open_sftp().await.unwrap_or_else(|e| {
        use std::error::Error;
        panic!("SFTP: {e}, {:?}", e.source());
    })));
    RemoteFixture {
        _server: server,
        root,
        ssh,
        remote,
        auth,
    }
}
fn path(root: &std::path::Path, name: &str) -> String {
    root.join(name).to_string_lossy().into_owned()
}

#[tokio::test]
async fn recursive_upload_download_and_remote_copy_preserve_binary_bytes() {
    let f = fixture().await;
    let local = tempfile::tempdir().expect("local");
    let source = path(local.path(), "한글 폴더");
    std::fs::create_dir_all(format!("{source}/nested")).expect("directory");
    let bytes: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    std::fs::write(format!("{source}/nested/a 'b.bin"), &bytes).expect("file");
    let target = path(f.root.path(), "upload");
    let control = TransferControl::default();
    let receipt = files::transfer(
        &FileSystem::Local,
        &source,
        &f.remote,
        &target,
        Conflict::Ask,
        false,
        &control,
        |_| {},
    )
    .await
    .expect("upload");
    assert_eq!(receipt.bytes, bytes.len() as u64);
    let copied = path(f.root.path(), "copy");
    files::transfer(
        &f.remote,
        &target,
        &f.remote,
        &copied,
        Conflict::Ask,
        false,
        &control,
        |_| {},
    )
    .await
    .expect("remote copy");
    let downloaded = path(local.path(), "download");
    files::transfer(
        &f.remote,
        &copied,
        &FileSystem::Local,
        &downloaded,
        Conflict::Ask,
        false,
        &control,
        |_| {},
    )
    .await
    .expect("download");
    assert_eq!(
        std::fs::read(format!("{downloaded}/nested/a 'b.bin")).expect("downloaded file"),
        bytes
    );
}

#[tokio::test]
async fn sftp_remains_live_after_the_shared_terminal_exits_without_reauthentication() {
    let f = fixture().await;
    let before = f.remote.list(".").await.expect("home").path;
    let spec = zerocode_core::PtySpec::remote(
        "sh",
        &[],
        zerocode_core::RemotePath::parse(&before).expect("root"),
        &[],
        24,
        80,
    );
    let mut pty = f
        .ssh
        .clone()
        .open_shared_pty(&spec)
        .await
        .expect("shared terminal");
    for _ in 0..50 {
        if pty.pump().ended {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(pty);
    assert_eq!(
        f.remote.list(".").await.expect("SFTP survives").path,
        before
    );
    assert_eq!(f.auth.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn resume_checks_the_prefix_and_conflicts_do_not_destroy_existing_files() {
    let f = fixture().await;
    let local = tempfile::tempdir().expect("local");
    let source = path(local.path(), "file");
    let target = path(f.root.path(), "file");
    std::fs::write(&source, b"abcdefghi").expect("source");
    std::fs::write(&target, b"abc").expect("partial");
    let control = TransferControl::default();
    files::transfer(
        &FileSystem::Local,
        &source,
        &f.remote,
        &target,
        Conflict::Resume,
        false,
        &control,
        |_| {},
    )
    .await
    .expect("resume");
    assert_eq!(std::fs::read(&target).expect("resumed"), b"abcdefghi");
    std::fs::write(&target, b"xyz").expect("different prefix");
    assert!(
        files::transfer(
            &FileSystem::Local,
            &source,
            &f.remote,
            &target,
            Conflict::Resume,
            false,
            &control,
            |_| {}
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&target).expect("preserved"), b"xyz");
    assert!(
        files::transfer(
            &FileSystem::Local,
            &source,
            &f.remote,
            &target,
            Conflict::Ask,
            false,
            &control,
            |_| {}
        )
        .await
        .is_err()
    );
    files::transfer(
        &FileSystem::Local,
        &source,
        &f.remote,
        &target,
        Conflict::Overwrite,
        false,
        &control,
        |_| {},
    )
    .await
    .expect("overwrite");
    assert_eq!(std::fs::read(&target).expect("overwritten"), b"abcdefghi");
}

#[tokio::test]
async fn rename_permissions_edit_conflicts_and_recursive_delete_use_sftp() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture().await;
    let dir = path(f.root.path(), "work");
    f.remote.mkdir(&dir).await.expect("mkdir");
    let file = format!("{dir}/hello.txt");
    f.remote
        .write_text(&file, "안녕", None)
        .await
        .expect("create text");
    assert!(
        f.remote
            .write_text(&file, "lost", Some("stale"))
            .await
            .is_err()
    );
    f.remote
        .write_text(&file, "저장", Some("안녕"))
        .await
        .expect("save");
    let renamed = format!("{dir}/new.txt");
    f.remote.rename(&file, &renamed).await.expect("rename");
    f.remote
        .chmod(&renamed, 0o600, false, &TransferControl::default())
        .await
        .expect("chmod");
    assert_eq!(
        std::fs::metadata(&renamed)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(f.remote.read_text(&renamed).await.expect("read"), "저장");
    let outside = path(f.root.path(), "outside");
    std::fs::write(&outside, "keep").expect("outside");
    std::os::unix::fs::symlink(&outside, format!("{dir}/link")).expect("symlink");
    f.remote
        .remove(&dir, &TransferControl::default())
        .await
        .expect("recursive remove");
    assert_eq!(
        std::fs::read_to_string(&outside).expect("link target survives"),
        "keep"
    );
}

#[tokio::test]
async fn cancellation_preserves_source_and_copy_into_self_is_rejected() {
    let f = fixture().await;
    let local = tempfile::tempdir().expect("local");
    let source = path(local.path(), "source");
    std::fs::write(&source, vec![7; 200_000]).expect("source");
    let target = path(f.root.path(), "target");
    let control = TransferControl::default();
    control.cancelled.store(true, Ordering::Relaxed);
    assert!(
        files::transfer(
            &FileSystem::Local,
            &source,
            &f.remote,
            &target,
            Conflict::Ask,
            true,
            &control,
            |_| {}
        )
        .await
        .is_err()
    );
    assert!(std::path::Path::new(&source).exists());
    assert!(!std::path::Path::new(&target).exists());
    let remote_root = f.remote.list(".").await.expect("canonical root").path;
    assert!(
        files::transfer(
            &f.remote,
            &remote_root,
            &f.remote,
            &format!("{remote_root}/inside"),
            Conflict::Ask,
            false,
            &TransferControl::default(),
            |_| {}
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn system_ssh_style_process_transport_uses_the_same_file_operations() {
    let root = tempfile::tempdir().expect("root");
    let binary = std::env::var_os("SFTP_SERVER")
        .map(PathBuf::from)
        .or_else(|| {
            [
                "/usr/libexec/sftp-server",
                "/usr/lib/openssh/sftp-server",
                "/usr/lib/ssh/sftp-server",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        })
        .expect("SFTP server");
    let child = tokio::process::Command::new(binary)
        .arg("-d")
        .arg(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("process");
    let transport = zerocode_ssh::ProcessSftp::open(child)
        .await
        .expect("SFTP process");
    let directory = transport.files.list(".").await.expect("home");
    assert_eq!(
        std::path::Path::new(&directory.path),
        root.path().canonicalize().expect("root")
    );
    assert!(!transport.is_closed());
    let file = format!("{}/file.txt", directory.path);
    transport
        .files
        .write_text(&file, "process transport", None)
        .await
        .expect("write");
    assert_eq!(
        transport.files.read_text(&file).await.expect("read"),
        "process transport"
    );
}

#[tokio::test]
async fn cancellation_during_streaming_can_be_resumed_without_corrupting_bytes() {
    let f = fixture().await;
    let local = tempfile::tempdir().expect("local");
    let source = path(local.path(), "source");
    let target = path(f.root.path(), "partial");
    let bytes = vec![19; 200_000];
    std::fs::write(&source, &bytes).expect("source");
    let control = TransferControl::default();
    control.bytes_per_second.store(1024, Ordering::Relaxed);
    let result = files::transfer(
        &FileSystem::Local,
        &source,
        &f.remote,
        &target,
        Conflict::Ask,
        false,
        &control,
        |p| {
            if p.bytes > 0 {
                control.cancelled.store(true, Ordering::Relaxed);
            }
        },
    )
    .await;
    assert!(matches!(result, Err(files::FileError::Cancelled)));
    files::transfer(
        &FileSystem::Local,
        &source,
        &f.remote,
        &target,
        Conflict::Resume,
        false,
        &TransferControl::default(),
        |_| {},
    )
    .await
    .expect("resume cancelled transfer");
    assert_eq!(std::fs::read(&target).expect("target"), bytes);
}

#[tokio::test]
async fn recursive_permissions_change_children_before_locking_the_directory() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture().await;
    let directory = f.root.path().join("permissions");
    std::fs::create_dir(&directory).expect("directory");
    std::fs::write(directory.join("child"), "data").expect("child");
    let result = f
        .remote
        .chmod(
            &directory.to_string_lossy(),
            0o400,
            true,
            &TransferControl::default(),
        )
        .await;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("restore parent for cleanup");
    assert!(result.is_ok(), "recursive permissions: {result:?}");
    assert_eq!(
        std::fs::metadata(directory.join("child"))
            .expect("child metadata")
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
}
