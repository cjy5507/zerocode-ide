//! SFTP over an existing system SSH transport (including config aliases and
//! jump hosts). The child owns the subsystem, while SSH owns authentication.
use crate::{ConnectError, files::FileSystem};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct ProcessStream {
    input: ChildStdin,
    output: ChildStdout,
}
impl AsyncRead for ProcessStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.output).poll_read(cx, buffer)
    }
}
impl AsyncWrite for ProcessStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.input).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.input).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.input).poll_shutdown(cx)
    }
}

pub struct ProcessSftp {
    process: Mutex<Child>,
    pub files: FileSystem,
}
impl ProcessSftp {
    pub async fn open(mut process: Child) -> Result<Arc<Self>, ConnectError> {
        let input = process.stdin.take().ok_or(ConnectError::ChannelProtocol)?;
        let output = process.stdout.take().ok_or(ConnectError::ChannelProtocol)?;
        let session = tokio::time::timeout(
            crate::deadline::PTY_SETUP,
            russh_sftp::client::SftpSession::new(ProcessStream { input, output }),
        )
        .await
        .map_err(|_| ConnectError::Timeout)??;
        Ok(Arc::new(Self {
            process: Mutex::new(process),
            files: FileSystem::Remote(Arc::new(session)),
        }))
    }
    pub fn is_closed(&self) -> bool {
        !matches!(
            self.process
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_wait(),
            Ok(None)
        )
    }
}
