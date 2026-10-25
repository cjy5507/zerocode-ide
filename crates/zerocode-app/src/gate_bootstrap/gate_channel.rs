use std::io;
use std::net::TcpStream;
use std::process::{Child, Command};

pub fn spawn(mut command: Command, channel: TcpStream) -> io::Result<Child> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let fd = channel.as_raw_fd();
    if fd <= 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private gate descriptor overlaps standard IO",
        ));
    }
    // SAFETY: channel owns fd through spawn; fcntl is async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}
