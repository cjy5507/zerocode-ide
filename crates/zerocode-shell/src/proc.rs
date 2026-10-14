//! 창이 자식을 띄우는 한 문.
//!
//! release 의 `main.rs` 는 `windows_subsystem = "windows"` 로 서므로 창에는
//! 콘솔이 없다. 그 아래에서 콘솔 자식(`git`, `gh`, `security`, `adb`,
//! `powershell`…)을 맨 `Command::new` 로 띄우면 윈도우는 자식마다 새 콘솔
//! 창을 만들고, 사람은 화면 한가운데에서 검은 창이 깜빡이는 것을 본다
//! (AO #5309). `CREATE_NO_WINDOW` 하나면 그 창이 생기지 않는다 — 자식의
//! 인자도 환경도 cwd 도 stdio 도 그대로다.
//!
//! 그래서 이 문은 **플래그만** 얹는다. 나머지는 부르는 쪽이 하던 그대로
//! `Command` 에 얹으므로, 116 곳을 옮기는 일이 동작을 바꾸지 않는다.
//! 크레이트의 소스 계약(`tests/source_contracts/quiet_children.rs`)이 맨
//! `Command::new(` 가 이 파일 밖에 남지 않았음을 고정한다.
//!
//! unix 에서는 `Command::new` 를 그대로 돌려준다 — 플래그도 분기도 없고,
//! 호출 비용은 `Command::new` 그 자체다.

use std::ffi::OsStr;

/// 콘솔 창 없이 자식을 시작한다 (`winbase.h` 의 `CREATE_NO_WINDOW`).
///
/// 이 크레이트에서 이 상수를 적는 곳은 여기 한 줄뿐이다.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `std::process::Command::new` — windows 에서는 콘솔 창을 띄우지 않는다.
pub(crate) fn quiet_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)] // unix 에서는 얹을 것이 없다.
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 같은 문의 tokio 판. `tokio::process::Command` 는 제 `creation_flags` 를
/// 들고 있으므로 트레이트를 들일 것이 없다.
pub(crate) fn quiet_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 자식에게 제 `PATH` 를 주는 문 — 이름은 그 `PATH` 위에서 먼저 풀린다.
///
/// std 의 `Command::spawn` 은 프로그램이 경로이거나 명령이 `PATH` 를 건드리지
/// 않았을 때만 `posix_spawn` 을 타고, 맨 이름에 제 `PATH` 를 준 명령은
/// `fork` 로 떨어진다(`library/std/src/sys/process/unix`: `env_saw_path()
/// && !program_is_path()`). 창의 fork 는 exec 전에 CEF/XPC 의 atfork
/// 처리기 안에서 영원히 멈출 수 있다(t-47141). 그래서 unix 에서는 맨 이름을
/// `execvp` 가 했을 걸음 그대로 — `path` 의 디렉터리 차례로, 실행 비트가 선
/// 보통 파일 — 절대 경로로 풀어 넘기고, 못 찾으면 첫 디렉터리 아래 그 이름을
/// 넘겨 `posix_spawn` 이 `NotFound` 로 답하게 한다. 어느 쪽이든 fork 는 없다.
/// windows 는 `CreateProcess` 라 이름을 그대로 둔다. `path` 가 `None` 이면
/// 자식은 이 프로세스의 `PATH` 를 물려받는다 — `PATH` 를 건드리지 않았으니
/// 그 길도 `posix_spawn` 이다.
pub(crate) fn quiet_command_on_path(
    program: impl AsRef<OsStr>,
    path: Option<impl AsRef<OsStr>>,
) -> std::process::Command {
    let Some(path) = path else {
        return quiet_command(program);
    };
    let path = path.as_ref();
    #[cfg(unix)]
    let mut command = quiet_command(resolve_on_path(program.as_ref(), path));
    #[cfg(not(unix))]
    let mut command = quiet_command(program);
    command.env("PATH", path);
    command
}

/// `execvp` 의 걸음: 이름에 `/` 가 있으면 그대로, 없으면 `path` 의 디렉터리
/// 차례로 실행 비트가 선 보통 파일. 없으면 첫 디렉터리(없으면 루트) 아래
/// 그 이름 — 존재하지 않는 절대 경로라 spawn 이 `NotFound` 로 답한다.
///
/// `execvp` 와 다른 데가 둘 있다(t-47457).
/// - 실행 비트는 `mode & 0o111` 로 본다: 누구의 비트든 하나만 서도 고른다.
///   그래서 다른 사용자만 실행할 수 있는 파일도 골라지고, spawn 은
///   `PermissionDenied` 로 답한다. `execvp` 는 `EACCES` 를 받으면 다음
///   디렉터리로 넘어간다.
/// - 빈 `PATH` 항목은 건너뛴다. `execvp` 는 빈 항목을 현재 디렉터리로
///   읽지만, 창의 자식은 어느 cwd 에서 시작하든 그 자리에 놓인 파일을
///   실행하지 않게 일부러 이렇게 한다.
///
/// 둘 다 제품의 `PATH` 에는 해당하지 않는다. 로그인 셸에서 읽은 `PATH` 는
/// 빈 항목을 거른 값이고(`shell_path::parse_captured`·`merge`), 고정된
/// `/usr/bin:/bin` 에도 빈 항목이 없다. 실행 비트의 차이는 남만 실행할 수
/// 있는 같은 이름의 파일이 사람이 설치한 도구(`gh`·`glab`·`git`·에이전트
/// CLI)보다 앞 디렉터리에 놓여야 드러난다.
#[cfg(unix)]
fn resolve_on_path(program: &OsStr, path: &OsStr) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    if program.as_bytes().contains(&b'/') {
        return std::path::PathBuf::from(program);
    }
    let dirs: Vec<std::path::PathBuf> = std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    dirs.iter()
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            dirs.first()
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(program)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 문은 플래그만 얹는다: 같은 program·args·env·cwd 를 준 두 `Command` 는
    /// 프로그램도 인자도 환경도 작업 디렉터리도 글자까지 같다. `Command` 의
    /// `Debug` 가 그 넷을 모두 적으므로, 한 번의 비교가 넷을 다 본다.
    #[test]
    fn the_door_changes_nothing_but_the_flag() {
        let build = |mut command: std::process::Command| {
            command
                .args(["--one", "두"])
                .env("ZEROCODE_DOOR", "1")
                .env_remove("ZEROCODE_GONE")
                .current_dir("/tmp");
            format!("{command:?}")
        };
        assert_eq!(
            build(quiet_command("/bin/echo")),
            build(std::process::Command::new("/bin/echo")),
        );
        assert_eq!(
            format!("{:?}", quiet_tokio_command("/bin/echo").arg("hi")),
            format!("{:?}", tokio::process::Command::new("/bin/echo").arg("hi")),
        );
    }

    /// 그리고 정말로 같은 아이가 돌아온다: 종료 코드도, stdout 도, 물려받은
    /// 환경과 cwd 도. (플래그가 있는 판은 windows 에서만 다르고, 그 차이는
    /// 창이 생기지 않는다는 것뿐이다.)
    #[cfg(unix)]
    #[test]
    fn the_child_runs_exactly_as_before() {
        let run = |mut command: std::process::Command| {
            let out = command
                .args(["-c", "printf %s \"$ZEROCODE_DOOR:$(pwd)\"; exit 3"])
                .env("ZEROCODE_DOOR", "opened")
                .current_dir("/tmp")
                .stdin(std::process::Stdio::null())
                .output()
                .expect("the child ran");
            (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        };
        let through_the_door = run(quiet_command("/bin/sh"));
        assert_eq!(through_the_door, run(std::process::Command::new("/bin/sh")));
        assert_eq!(through_the_door.0, Some(3));
        assert!(through_the_door.1.starts_with("opened:"));
    }

    /// 생성 비용 — 문이 `Command::new` 보다 느려지면 안 된다. 숫자를 적으려고
    /// 재는 것이지 게이트가 아니므로 `--ignored` 로만 돈다:
    /// `cargo test -p zerocode-shell --bins proc::tests::creation_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a gate"]
    fn creation_cost() {
        const ROUNDS: u32 = 200_000;
        let bare = std::time::Instant::now();
        for _ in 0..ROUNDS {
            std::hint::black_box(std::process::Command::new("/bin/echo"));
        }
        let bare = bare.elapsed();
        let door = std::time::Instant::now();
        for _ in 0..ROUNDS {
            std::hint::black_box(quiet_command("/bin/echo"));
        }
        let door = door.elapsed();
        println!(
            "Command::new {:?}/call, quiet_command {:?}/call",
            bare / ROUNDS,
            door / ROUNDS
        );
    }

    /// 제 `PATH` 를 받은 자식은 OS 에 절대 경로로 넘어간다 — `execvp` 의
    /// 걸음대로 `PATH` 차례의 첫 실행 파일이고, 실행 비트 없는 같은 이름은
    /// 지나친다. 그래서 std 는 fork 가 아니라 `posix_spawn` 을 탄다(t-47141).
    /// `PATH` 자체도 자식의 환경에 그대로 선다.
    #[cfg(unix)]
    #[test]
    fn a_child_given_a_path_is_named_by_the_first_executable_on_it() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("a temp dir");
        let plain = root.path().join("plain");
        let real = root.path().join("real");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(plain.join("zo-probe"), "#!/bin/sh\n").unwrap();
        std::fs::write(real.join("zo-probe"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(
            real.join("zo-probe"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let path = std::env::join_paths([plain.as_path(), real.as_path()]).unwrap();

        let command = quiet_command_on_path("zo-probe", Some(&path));
        assert_eq!(command.get_program(), real.join("zo-probe").as_os_str());
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "PATH" && value == Some(path.as_os_str())),
            "the child keeps the PATH it was given: {command:?}"
        );
    }

    /// 이름이 `PATH` 어디에도 없으면 첫 디렉터리 아래 그 이름 — 존재하지
    /// 않는 절대 경로다. spawn 은 `NotFound` 로 답하고, fork 는 없다. 이미
    /// 경로인 프로그램은 손대지 않고, `PATH` 를 주지 않으면 문은 이름도
    /// 환경도 그대로 둔다.
    #[cfg(unix)]
    #[test]
    fn a_name_not_on_the_path_still_becomes_an_absolute_path_and_a_path_stays_itself() {
        let root = tempfile::tempdir().expect("a temp dir");
        let path = std::env::join_paths([root.path()]).unwrap();

        let mut missing = quiet_command_on_path("zo-nowhere", Some(&path));
        assert_eq!(
            missing.get_program(),
            root.path().join("zo-nowhere").as_os_str()
        );
        assert!(matches!(
            missing.output().map(|_| ()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));

        let given = quiet_command_on_path("/bin/echo", Some(&path));
        assert_eq!(given.get_program(), "/bin/echo");

        let inherited = quiet_command_on_path("zo-nowhere", None::<&str>);
        assert_eq!(inherited.get_program(), "zo-nowhere");
        assert_eq!(inherited.get_envs().count(), 0);
    }
}
