use std::fs;
use std::path::Path;
use std::process::Command;

fn invoke(options: &[String], root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zerocode"))
        .arg("gate-bootstrap")
        .args(options)
        .args(["--", "--fixture", "--bind", "forwarded-value", "--help"])
        .env_clear()
        .env("HOME", root)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PYTHONPATH", root.join("candidate"))
        .env("PYTHONHOME", root.join("candidate"))
        .output()
        .unwrap()
}

#[test]
fn real_python_uses_retained_launcher_bytes_and_declared_standard_library() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let launcher = root.join("launcher.py");
    fs::write(&launcher,
        "import json, pathlib, sys\nassert sys.flags.isolated and sys.flags.no_site\nassert sys.argv[-3:] == ['--bind', 'forwarded-value', '--help']\nassert pathlib.Path(json.__file__).is_relative_to(pathlib.Path(sys.executable).parent.parent)\nassert __file__ == '<stdin>'\npathlib.Path(sys.argv[2]).write_text(\"raise AssertionError('replacement executed')\\n\")\nprint('verified retained launcher ran')\n").unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/release/tests/gate_bootstrap_fixture.py");
    let output = Command::new("python3")
        .args(["-I", "-S", "-B"])
        .arg(helper)
        .arg("--root")
        .arg(&root)
        .arg("--launcher")
        .arg(&launcher)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let options: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    let original = fs::read(&launcher).unwrap();
    let output = invoke(&options, &root);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("verified retained launcher ran"));
    assert!(
        fs::read_to_string(&launcher)
            .unwrap()
            .contains("replacement executed")
    );
    fs::write(&launcher, &original).unwrap();
    let python =
        Path::new(&options[options.iter().position(|word| word == "--python").unwrap() + 1]);
    let runtime =
        Path::new(&options[options.iter().position(|word| word == "--stdlib").unwrap() + 1]);
    let module = fs::read_dir(runtime.join("lib"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("json/__init__.py");
    let original_module = fs::read(&module).unwrap();
    fs::write(&module, "raise AssertionError('changed stdlib executed')\n").unwrap();
    assert_eq!(invoke(&options, &root).status.code(), Some(1));
    assert_eq!(fs::read(&launcher).unwrap(), original);
    fs::write(&module, original_module).unwrap();
    let original_python = fs::read(python).unwrap();
    fs::write(python, b"changed interpreter bytes").unwrap();
    assert_eq!(invoke(&options, &root).status.code(), Some(1));
    assert_eq!(fs::read(&launcher).unwrap(), original);
    fs::write(python, original_python).unwrap();
    fs::write(
        python
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("pyvenv.cfg"),
        "home = candidate\n",
    )
    .unwrap();
    assert_eq!(invoke(&options, &root).status.code(), Some(1));
    fs::remove_file(
        python
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("pyvenv.cfg"),
    )
    .unwrap();

    for flag in ["--launcher-sha256", "--python-sha256", "--stdlib-sha256"] {
        let mut changed = options.clone();
        let index = changed.iter().position(|word| word == flag).unwrap() + 1;
        changed[index] = "0".repeat(64);
        let output = invoke(&changed, &root);
        assert_eq!(output.status.code(), Some(1));
        let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(answer["decision"], "miss");
        assert_eq!(answer["newExecutions"], 0);
        assert_eq!(fs::read(&launcher).unwrap(), original);
    }
    fs::write(
        &launcher,
        "raise AssertionError('changed launcher executed')\n",
    )
    .unwrap();
    let output = invoke(&options, &root);
    assert_eq!(output.status.code(), Some(1));
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
    fs::write(&launcher, &original).unwrap();
    let output = invoke(&options, &root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
