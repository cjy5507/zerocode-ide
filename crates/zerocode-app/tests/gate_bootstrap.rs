#[cfg(target_os = "macos")]
use std::fs;
#[cfg(any(target_os = "macos", not(unix)))]
use std::path::Path;
#[cfg(any(target_os = "macos", not(unix)))]
use std::process::Command;

#[cfg(any(target_os = "macos", not(unix)))]
fn invoke(options: &[String], root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zerocode"))
        .arg("gate-bootstrap")
        .args(options)
        .args(["--", "--fixture"])
        .arg(&options[options.iter().position(|word| word == "--python").unwrap() + 1])
        .args(["--bind", "forwarded-value", "--help"])
        .env_clear()
        .env("HOME", root)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PYTHONPATH", root.join("candidate"))
        .env("PYTHONHOME", root.join("candidate"))
        .env("ZEROCODE_AGENT_TEAM_TOKEN", "fixture-parent-capability")
        .env(
            "ZEROCODE_AGENT_TEAM_TOKEN_FILE",
            root.join("fixture-parent-authority"),
        )
        .env("ZEROCODE_HOOK_TOKEN", "fixture-parent-bridge-capability")
        .output()
        .unwrap()
}

#[test]
// The retained runtime verifier walks Mach-O load commands with otool.
// Other platforms deliberately refuse this runtime before launching Python.
#[cfg(target_os = "macos")]
fn real_python_uses_retained_launcher_bytes_and_declared_standard_library() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let launcher = root.join("launcher.py");
    let candidate = root.join("candidate");
    fs::create_dir(&candidate).unwrap();
    let marker = root.join("candidate-imported");
    fs::write(
        candidate.join("json.py"),
        format!(
            "from pathlib import Path\nPath({:?}).write_text('imported')\n",
            marker.to_str().unwrap()
        ),
    )
    .unwrap();
    let control = Command::new("python3")
        .args(["-S", "-B", "-c", "import json"])
        .env_clear()
        .env("HOME", &root)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PYTHONPATH", &candidate)
        .output()
        .unwrap();
    assert!(
        control.status.success(),
        "{}",
        String::from_utf8_lossy(&control.stderr)
    );
    assert!(
        marker.is_file(),
        "candidate import positive control never ran"
    );
    fs::remove_file(&marker).unwrap();
    fs::write(&launcher,
        "import encodings, json, os, pathlib, sys\nassert not any(key in os.environ for key in ['ZEROCODE_AGENT_TEAM_TOKEN', 'ZEROCODE_AGENT_TEAM_TOKEN_FILE', 'ZEROCODE_HOOK_TOKEN'])\nassert sys.flags.isolated and sys.flags.no_site\nassert sys.argv[-3:] == ['--bind', 'forwarded-value', '--help']\nassert sys.argv[3] == '--fixture'\npython = pathlib.Path(sys.argv[4]).resolve()\nassert pathlib.Path(sys.executable).resolve() == python, 'reported executable differs from pinned Python'\nassert pathlib.Path(sys.orig_argv[0]).resolve() == python, 'actual process image differs from pinned Python'\nassert pathlib.Path(sys._base_executable).resolve() == python, 'base executable differs from pinned Python'\nroot = python.parent.parent\nstdlib = root / 'lib' / ('python%d.%d' % sys.version_info[:2])\nassert pathlib.Path(json.__file__).is_relative_to(stdlib)\nassert pathlib.Path(encodings.__file__).is_relative_to(stdlib)\nassert sys.path == [str(stdlib), str(stdlib / 'lib-dynload')]\nassert __file__ == '<stdin>'\npathlib.Path(sys.argv[2]).write_text(\"raise AssertionError('replacement executed')\\n\")\nprint('verified retained launcher ran')\n").unwrap();
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
        !marker.exists(),
        "candidate module polluted the verified runtime"
    );
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
    let startup = std::path::PathBuf::from(format!("{}._pth", python.display()));
    let startup_bytes = fs::read(&startup).unwrap();
    let misnamed = python.with_extension("_pth");
    fs::rename(&startup, &misnamed).unwrap();
    let without = Command::new(python)
        .args(["-I", "-S", "-B", "-c", "import encodings,json,pathlib,sys; root=pathlib.Path(sys.executable).parent.parent; print(all(pathlib.Path(module.__file__).is_relative_to(root) for module in (encodings,json)))"])
        .env_clear()
        .env("HOME", &root)
        .output()
        .unwrap();
    assert!(
        without.status.success(),
        "{}",
        String::from_utf8_lossy(&without.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&without.stdout).trim(),
        "False",
        "misnamed startup file did not expose the original framework library"
    );
    let denied = invoke(&options, &root);
    assert_eq!(denied.status.code(), Some(1));
    assert_eq!(fs::read(&launcher).unwrap(), original);
    fs::rename(&misnamed, &startup).unwrap();
    for changed in [
        b"".as_slice(),
        b"import site\n".as_slice(),
        b"../outside\n../other\n".as_slice(),
    ] {
        fs::write(&startup, changed).unwrap();
        assert_eq!(invoke(&options, &root).status.code(), Some(1));
        assert_eq!(fs::read(&launcher).unwrap(), original);
    }
    fs::write(&startup, startup_bytes).unwrap();
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
        runtime.parent().unwrap().join("pyvenv.cfg"),
        "home = candidate\n",
    )
    .unwrap();
    assert_eq!(invoke(&options, &root).status.code(), Some(1));
    fs::remove_file(runtime.parent().unwrap().join("pyvenv.cfg")).unwrap();

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

#[cfg(not(unix))]
#[test]
fn an_authorized_gate_job_on_an_unsupported_platform_executes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let mut options = Vec::new();
    for flag in ["--launcher", "--python", "--stdlib"] {
        options.extend([
            flag.to_string(),
            root.path().join("absent").display().to_string(),
        ]);
        options.extend([format!("{flag}-sha256"), "0".repeat(64)]);
    }
    options.extend(["--gate-job".to_string(), "fixture-job".to_string()]);
    let output = invoke(&options, root.path());
    assert_eq!(output.status.code(), Some(1));
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["decision"], "miss");
    assert_eq!(answer["code"], "unsupported_platform");
    assert_eq!(answer["executed"], false);
    assert_eq!(answer["newExecutions"], 0);
}
