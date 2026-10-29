//! The environment a process was started with, read from the kernel rather
//! than asked of the process (t-81714): what its parent handed it, which
//! the process keeps when that parent is gone. Only a process of the same
//! user reads; another user's, and every process on a host with no way to
//! read one (Windows), is not known. macOS leaves the environment out for
//! its own programs — `/bin/sleep`, `/bin/zsh`, `/usr/bin/tail` — whoever
//! asks: on 10-29 478 of this machine's 582 processes of one user read with
//! their arguments only, so they name nothing.

/// Reads processes' environments, one buffer for all of them.
pub(super) struct Environs {
    buffer: Vec<u8>,
}

impl Environs {
    pub(super) fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// The environment `pid` was started with; `None` when it does not read.
    pub(super) fn of(&mut self, pid: u32) -> Option<Environ<'_>> {
        read_into(pid, &mut self.buffer).map(|words| Environ { words })
    }
}

/// `NAME=value` words, each ended by a NUL, up to the first empty one.
pub(super) struct Environ<'a> {
    words: &'a [u8],
}

impl Environ<'_> {
    /// `name`'s value — the first one given, as the process's own `getenv`
    /// finds it.
    pub(super) fn var(&self, name: &str) -> Option<String> {
        self.words
            .split(|&byte| byte == 0)
            .take_while(|word| !word.is_empty())
            .find_map(|word| word.strip_prefix(name.as_bytes())?.strip_prefix(b"="))
            .map(|value| String::from_utf8_lossy(value).into_owned())
    }
}

/// `KERN_PROCARGS2`: the process's arguments and environment as its stack
/// was laid out at `exec`.
#[cfg(target_os = "macos")]
fn read_into(pid: u32, buffer: &mut Vec<u8>) -> Option<&[u8]> {
    use nix::libc;

    if buffer.is_empty() {
        // Zeroed pages from the allocator, not a megabyte written byte by
        // byte: a judgment reads two or three environments, not hundreds.
        *buffer = vec![0; args_max()?];
    }
    let mut name = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        i32::try_from(pid).ok()?,
    ];
    let mut size = buffer.len();
    // SAFETY: `name` holds the three ints its length says; the kernel writes
    // at most `size` bytes — the buffer's own length — and sets `size` to how
    // many it wrote; no new value is handed to it.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    environ_in_procargs(buffer.get(..size)?)
}

/// The most one process's arguments and environment may hold
/// (`kern.argmax`): the room [`read_into`]'s buffer needs for any of them.
#[cfg(target_os = "macos")]
fn args_max() -> Option<usize> {
    use nix::libc;

    let mut name = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: `name` holds the two ints its length says; the kernel writes at
    // most `size` bytes, one `c_int`, into `value`.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            2,
            (&raw mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    usize::try_from(value).ok().filter(|max| *max > 0)
}

/// `/proc/<pid>/environ`: the environment as it was laid out at `exec`.
#[cfg(target_os = "linux")]
fn read_into(pid: u32, buffer: &mut Vec<u8>) -> Option<&[u8]> {
    use std::io::Read as _;

    buffer.clear();
    std::fs::File::open(format!("/proc/{pid}/environ"))
        .ok()?
        .read_to_end(buffer)
        .ok()?;
    Some(buffer.as_slice())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_into(_pid: u32, _buffer: &mut Vec<u8>) -> Option<&[u8]> {
    None
}

/// The environment in a `KERN_PROCARGS2` buffer: a C int argc, the
/// executable's path and the NULs that pad it, the argc argument words,
/// then the environment words — ended by an empty word, past which the
/// kernel's own words (`ptr_munge=…`, `executable_file=…`) follow and are
/// not the process's environment. A buffer that ends early reads as
/// nothing.
#[cfg(any(target_os = "macos", test))]
fn environ_in_procargs(buffer: &[u8]) -> Option<&[u8]> {
    let (argc, rest) = buffer.split_first_chunk::<4>()?;
    let argc = usize::try_from(i32::from_ne_bytes(*argc)).ok()?;
    let path_end = rest.iter().position(|&byte| byte == 0)?;
    let mut at = path_end + rest[path_end..].iter().position(|&byte| byte != 0)?;
    for _ in 0..argc {
        at += rest[at..].iter().position(|&byte| byte == 0)? + 1;
    }
    rest.get(at..)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `KERN_PROCARGS2` buffer the way the kernel lays one out.
    fn procargs(path: &str, argv: &[&str], env: &[&str], apple: &[&str]) -> Vec<u8> {
        let argc = i32::try_from(argv.len()).expect("a handful of words");
        let mut buffer = argc.to_ne_bytes().to_vec();
        buffer.extend_from_slice(path.as_bytes());
        buffer.extend_from_slice(&[0; 4]);
        for word in argv.iter().chain(env).chain(&[""]).chain(apple) {
            buffer.extend_from_slice(word.as_bytes());
            buffer.push(0);
        }
        buffer
    }

    fn var(buffer: &[u8], name: &str) -> Option<String> {
        Environ {
            words: environ_in_procargs(buffer)?,
        }
        .var(name)
    }

    #[test]
    fn the_environment_is_read_past_the_arguments_and_before_the_kernels_words() {
        let buffer = procargs(
            "/usr/bin/python3",
            &["python3", "-m", "ZEROCODE_PANE_KEY=term-9", "http.server"],
            &[
                "LANG=C",
                "ZEROCODE_PANE_KEY=term-1",
                "ZEROCODE_PANE_KEY=term-2",
            ],
            &["executable_file=0x1", "ZEROCODE_WINDOW_PID=7"],
        );
        // An argument that looks like the variable is an argument; the first
        // value given is the one the process reads; the kernel's own words
        // after the environment are not the process's.
        assert_eq!(var(&buffer, "ZEROCODE_PANE_KEY").as_deref(), Some("term-1"));
        assert_eq!(var(&buffer, "LANG").as_deref(), Some("C"));
        assert_eq!(var(&buffer, "ZEROCODE_WINDOW_PID"), None);
        assert_eq!(var(&buffer, "ZEROCODE_PANE"), None);
    }

    #[test]
    fn a_buffer_that_ends_early_reads_as_nothing() {
        let whole = procargs("/bin/sleep", &["sleep", "600"], &["A=1"], &[]);
        for cut in [0, 3, 4, 10, 20] {
            assert_eq!(var(&whole[..cut], "A"), None, "{cut}");
        }
        assert_eq!(var(&whole, "A").as_deref(), Some("1"));
        let negative = [
            (-1_i32).to_ne_bytes().as_slice(),
            b"/bin/sh\0\0sh\0A=1\0".as_slice(),
        ]
        .concat();
        assert_eq!(var(&negative, "A"), None);
    }
}
