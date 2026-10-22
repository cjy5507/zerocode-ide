use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;

use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    endpoint: PathBuf,
    grant: PathBuf,
    token: PathBuf,
    program: PathBuf,
    env: BTreeMap<String, OsString>,
}

impl Fixture {
    fn new(windows: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let hooks = root.path().join("application/agent-hooks");
        fs::create_dir_all(hooks.join("panes")).unwrap();
        let endpoint = hooks.join("endpoint.env");
        fs::write(&endpoint, "window endpoint").unwrap();
        let token = hooks.join("private-token");
        fs::write(&token, "TOKEN_BYTES_NOT_FOR_A_DIAGNOSTIC").unwrap();
        let grant = hooks.join("panes/term-7.env");
        fs::write(&grant, format!(
            "ZEROCODE_AGENT_TEAM_ID='fixture-team'\nZEROCODE_AGENT_TEAM_PANE='%1'\nZEROCODE_AGENT_TEAM_TOKEN_FILE='{}'\n",
            token.to_string_lossy().replace('\'', "'\\''"),
        )).unwrap();
        let program = orchestration_program(&endpoint, windows).unwrap();
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "fixture command; never executed by this test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = BTreeMap::from([
            ("ZEROCODE_HOOK_ENDPOINT".into(), endpoint.as_os_str().to_owned()),
            ("ZEROCODE_PANE_KEY".into(), "term-7".into()),
        ]);
        Self { _root: root, endpoint, grant, token, program, env }
    }

    fn resolve(&self, windows: bool) -> Result<Door, Unavailable> {
        resolve(|key| self.env.get(key).cloned(), windows)
    }
}

#[test]
fn a_hand_started_process_finds_its_own_grant_and_installed_command() {
    let fixture = Fixture::new(false);
    let door = fixture.resolve(false).unwrap();
    assert_eq!(door.program, fixture.program);
    let mut command = Command::new(&door.program);
    command.env("ZEROCODE_AGENT_TEAM_ID", "an-incomplete-old-binding");
    command.env("ZEROCODE_AGENT_TEAM_TOKEN", "OLD_TOKEN_MUST_NOT_CROSS");
    door.apply(&mut command);
    let env: BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_ID")], Some(OsStr::new("fixture-team")));
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_PANE")], Some(OsStr::new("%1")));
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_TOKEN_FILE")], Some(fixture.token.as_os_str()));
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_TOKEN")], None);
}

#[test]
fn the_complete_launch_environment_does_not_need_a_file_grant() {
    let mut fixture = Fixture::new(false);
    fs::remove_file(&fixture.grant).unwrap();
    fixture.env.clear();
    fixture.env.extend([
        ("PATH".into(), fixture.program.parent().unwrap().as_os_str().to_owned()),
        ("ZEROCODE_AGENT_TEAM_ID".into(), "launched-team".into()),
        ("TMUX_PANE".into(), "%2".into()),
        ("ZEROCODE_AGENT_TEAM_TOKEN".into(), "launched-token".into()),
    ]);
    let door = fixture.resolve(false).unwrap();
    assert_eq!(door.program, fixture.program);
    let mut command = Command::new(&door.program);
    door.apply(&mut command);
    assert_eq!(command.get_envs().count(), 0, "the inherited binding was replaced");
}

#[test]
fn an_incomplete_environment_uses_one_complete_pane_binding() {
    let mut fixture = Fixture::new(false);
    fixture.env.insert("ZEROCODE_AGENT_TEAM_ID".into(), "incomplete-team".into());
    fixture.env.insert("ZEROCODE_AGENT_TEAM_TOKEN".into(), "incomplete-token".into());
    let door = fixture.resolve(false).unwrap();
    let mut command = Command::new(&door.program);
    door.apply(&mut command);
    let env: BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_ID")], Some(OsStr::new("fixture-team")));
    assert_eq!(env[OsStr::new("ZEROCODE_AGENT_TEAM_TOKEN")], None);
}

#[test]
fn a_revoked_or_different_pane_grant_is_not_reused() {
    let mut fixture = Fixture::new(false);
    fixture.env.insert("ZEROCODE_PANE_KEY".into(), "term-8".into());
    assert!(matches!(fixture.resolve(false), Err(Unavailable::NoGrant)));
    fixture.env.insert("ZEROCODE_PANE_KEY".into(), "term-7".into());
    assert!(fixture.resolve(false).is_ok());
    fs::remove_file(&fixture.grant).unwrap();
    assert!(matches!(fixture.resolve(false), Err(Unavailable::NoGrant)));
}

#[test]
fn diagnostics_distinguish_a_missing_pane_grant_command_and_bad_file() {
    let mut fixture = Fixture::new(false);
    fixture.env.remove("ZEROCODE_HOOK_ENDPOINT");
    assert!(matches!(fixture.resolve(false), Err(Unavailable::NoPane)));
    fixture.env.insert("ZEROCODE_HOOK_ENDPOINT".into(), fixture.endpoint.as_os_str().to_owned());
    fs::remove_file(&fixture.program).unwrap();
    assert!(matches!(fixture.resolve(false), Err(Unavailable::NoProgram)));
    fs::write(&fixture.grant, "ZEROCODE_AGENT_TEAM_ID=SECRET_NOT_FOR_ERRORS").unwrap();
    let Err(why) = fixture.resolve(false) else { panic!("bad grant accepted") };
    assert_eq!(why, Unavailable::UnreadableGrant(io::ErrorKind::InvalidData));
    assert!(!why.to_string().contains("SECRET_NOT_FOR_ERRORS"));
}

#[test]
fn the_windows_door_is_the_installed_powershell_script() {
    let fixture = Fixture::new(true);
    assert_eq!(fixture.resolve(true).unwrap().program, fixture.program);
}

#[cfg(unix)]
#[test]
fn a_non_executable_posix_file_is_not_a_ledger_command() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(false);
    fs::set_permissions(&fixture.program, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(fixture.resolve(false), Err(Unavailable::NoProgram)));
}
