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
}
