mod support;

use std::collections::HashMap;
use std::sync::Arc;

use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{Attrs, File, FileAttributes, Name, StatusCode, Version};
use tokio::sync::Mutex;
use zerocode_core::RemotePath;
use zerocode_ssh::{ConnectError, SshConnector, SshPassword};

use support::{ACCEPTED_PASSWORD, Fixture, USER};

#[derive(Default)]
struct WorkspaceServer {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl server::Handler for WorkspaceServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == USER && password == ACCEPTED_PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }

        let channel = self
            .channels
            .lock()
            .await
            .remove(&channel_id)
            .expect("fixture channel exists");
        session.channel_success(channel_id)?;
        russh_sftp::server::run(channel.into_stream(), WorkspaceSftp).await;
        Ok(())
    }
}

struct WorkspaceSftp;

impl russh_sftp::server::Handler for WorkspaceSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let canonical = match path.as_str() {
            "/srv/project-link" => "/srv/project",
            "/srv/project" => "/srv/project",
            "/srv/file" => "/srv/file",
            "/srv/invalid" => "relative/not-a-remote-root",
            _ => return Err(StatusCode::NoSuchFile),
        };
        Ok(Name {
            id,
            files: vec![File::dummy(canonical)],
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let mut attrs = FileAttributes::default();
        match path.as_str() {
            "/srv/project" => attrs.set_dir(true),
            "/srv/file" => attrs.set_regular(true),
            _ => return Err(StatusCode::NoSuchFile),
        }
        Ok(Attrs { id, attrs })
    }
}

async fn connection() -> (Fixture, zerocode_ssh::SshConnection) {
    let fixture = Fixture::start(WorkspaceServer::default()).await;
    let host = fixture.password_host(&fixture.public_key);
    let connection = SshConnector::default()
        .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())))
        .await
        .expect("fixture authenticates");
    (fixture, connection)
}

#[tokio::test]
async fn a_workspace_root_is_canonicalized_and_proven_to_be_a_directory() {
    let (_fixture, mut connection) = connection().await;
    let requested = RemotePath::parse("/srv/project-link").expect("valid remote root");

    let canonical = connection
        .verify_workspace_root(&requested)
        .await
        .expect("fixture directory verifies");

    assert_eq!(canonical.as_str(), "/srv/project");
    connection.disconnect().await.expect("disconnect fixture");
}

#[tokio::test]
async fn a_regular_remote_file_cannot_be_saved_as_a_workspace_root() {
    let (_fixture, mut connection) = connection().await;
    let requested = RemotePath::parse("/srv/file").expect("valid remote path");

    let error = connection
        .verify_workspace_root(&requested)
        .await
        .expect_err("a workspace root must be a directory");

    assert!(matches!(error, ConnectError::WorkspaceRootNotDirectory));
    connection.disconnect().await.expect("disconnect fixture");
}

#[tokio::test]
async fn a_server_cannot_canonicalize_a_workspace_into_an_unvalidated_path() {
    let (_fixture, mut connection) = connection().await;
    let requested = RemotePath::parse("/srv/invalid").expect("valid requested path");

    let error = connection
        .verify_workspace_root(&requested)
        .await
        .expect_err("server output must pass the remote path parser");

    assert!(matches!(error, ConnectError::InvalidWorkspaceRoot));
    connection.disconnect().await.expect("disconnect fixture");
}
