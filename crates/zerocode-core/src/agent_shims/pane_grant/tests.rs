use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    endpoint: PathBuf,
    grant: PathBuf,
    token: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application's data");
        let hooks = application.join("agent-hooks");
        fs::create_dir_all(hooks.join("panes")).unwrap();
        let endpoint = hooks.join("endpoint.env");
        fs::write(&endpoint, "window endpoint").unwrap();
        let token = application.join("private token");
        fs::write(&token, "TOKEN_BYTES_A_READER_MUST_NOT_COPY").unwrap();
        let grant = hooks.join("panes/term-7.env");
        let fixture = Self {
            _root: root,
            endpoint,
            grant,
            token,
        };
        fixture.write_grant();
        fixture
    }

    fn body(&self) -> String {
        let token = self.token.to_string_lossy().replace('\'', "'\\''");
        format!(
            "{TEAM_ID_VAR}='fixture-team'\n{TEAM_PANE_VAR}='%1'\n{TEAM_TOKEN_FILE_VAR}='{token}'\n"
        )
    }

    fn write_grant(&self) {
        fs::write(&self.grant, self.body()).unwrap();
    }

    fn read(&self) -> io::Result<Option<PaneTeamGrant>> {
        read_pane_team_grant(&self.endpoint, "term-7")
    }
}

#[test]
fn the_writer_quoting_keeps_spaces_and_apostrophes_in_token_paths() {
    let fixture = Fixture::new();
    let grant = fixture.read().unwrap().expect("own pane grant");
    let fields: BTreeMap<_, _> = grant.environment().into_iter().collect();
    assert_eq!(fields[TEAM_ID_VAR], "fixture-team");
    assert_eq!(fields[TEAM_PANE_VAR], "%1");
    assert_eq!(fields[TEAM_TOKEN_FILE_VAR], fixture.token.as_os_str());
    assert!(
        fields
            .values()
            .all(|value| *value != "TOKEN_BYTES_A_READER_MUST_NOT_COPY")
    );
}

#[test]
fn a_grant_for_another_pane_is_not_a_grant_for_this_one() {
    let fixture = Fixture::new();
    assert!(
        read_pane_team_grant(&fixture.endpoint, "term-8")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_removed_grant_or_token_cannot_keep_a_pane_authorized() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.grant).unwrap();
    assert!(fixture.read().unwrap().is_none());
    fixture.write_grant();
    fs::remove_file(&fixture.token).unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn pane_keys_are_exact_window_keys_and_never_paths() {
    let fixture = Fixture::new();
    for key in [
        "",
        "term-",
        "term-007",
        "term-7.extra",
        "other/term-7",
        "term-4294967296",
    ] {
        assert!(
            read_pane_team_grant(&fixture.endpoint, key).is_err(),
            "{key}"
        );
    }
    assert!(read_pane_team_grant(Path::new("relative/endpoint.env"), "term-7").is_err());
}

#[test]
fn browser_only_grants_do_not_create_a_team_binding() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.grant,
        "ZEROCODE_BROWSER_TOKEN_FILE='other-capability'\n",
    )
    .unwrap();
    assert!(fixture.read().unwrap().is_none());
}

#[test]
fn incomplete_duplicate_or_unquoted_fields_are_refused_without_quoting_them() {
    let fixture = Fixture::new();
    for body in [
        format!("{TEAM_ID_VAR}='fixture-team'\n"),
        fixture.body() + &format!("{TEAM_ID_VAR}='duplicate'\n"),
        format!("{TEAM_ID_VAR}=DO_NOT_QUOTE_THIS_VALUE\n"),
        format!("{TEAM_ID_VAR}='unterminated_DO_NOT_QUOTE_THIS_VALUE\n"),
    ] {
        fs::write(&fixture.grant, body).unwrap();
        let error = match fixture.read() {
            Err(error) => error,
            Ok(_) => panic!("bad grant accepted"),
        };
        assert!(!error.to_string().contains("DO_NOT_QUOTE_THIS_VALUE"));
    }
}

#[test]
fn reading_a_grant_is_bounded_and_requires_a_regular_file() {
    let fixture = Fixture::new();
    fs::write(&fixture.grant, "x".repeat(65 * 1024)).unwrap();
    assert!(fixture.read().is_err());
    fs::remove_file(&fixture.grant).unwrap();
    fs::create_dir(&fixture.grant).unwrap();
    assert!(fixture.read().is_err());
}

#[cfg(unix)]
#[test]
fn a_grant_does_not_follow_a_link_to_another_file() {
    let fixture = Fixture::new();
    let other = fixture.grant.with_extension("other");
    fs::rename(&fixture.grant, &other).unwrap();
    std::os::unix::fs::symlink(&other, &fixture.grant).unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn installed_commands_belong_to_the_window_the_endpoint_names() {
    let fixture = Fixture::new();
    let application = fixture.endpoint.parent().unwrap().parent().unwrap();
    assert_eq!(
        orchestration_program(&fixture.endpoint, false),
        Some(application.join("orchestration-bin/zerocode-orc"))
    );
    assert_eq!(
        orchestration_program(&fixture.endpoint, true),
        Some(application.join("orchestration-bin/zerocode-orc.ps1"))
    );
    assert!(orchestration_program(Path::new("endpoint.env"), false).is_none());
}
