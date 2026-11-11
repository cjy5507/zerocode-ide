//! The programs a command runs, as [`programs_run`] reaches them (t-104888):
//! the same walk the kill guard reads its calls through.

use super::*;

fn names(command: &str) -> Vec<String> {
    programs_run(command, |_| true)
        .into_iter()
        .map(|run| run.program)
        .collect()
}

#[test]
fn a_program_is_reached_through_operators_assignments_wrappers_and_paths() {
    assert_eq!(names("true && cliclick p"), ["true", "cliclick"]);
    assert_eq!(names("FOO=1 cliclick p"), ["cliclick"]);
    assert_eq!(
        names("sudo -u me nice -n 5 env A=1 cliclick p"),
        ["cliclick"]
    );
    assert_eq!(
        names("/opt/homebrew/bin/CliClick -m verbose c:1,1"),
        ["cliclick"]
    );
    assert_eq!(names("ls | xargs -n 1 cliclick"), ["ls", "cliclick"]);
}

#[test]
fn a_shell_string_an_eval_and_a_substitution_are_read_as_commands() {
    assert_eq!(names("bash -c 'cliclick p'"), ["cliclick"]);
    assert_eq!(names("eval cliclick p"), ["cliclick"]);
    assert_eq!(names("echo $(cliclick p)"), ["cliclick", "echo"]);
    assert_eq!(names("bash <<'EOF'\ncliclick p\nEOF"), ["cliclick"]);
    assert_eq!(
        names("echo 'cliclick p' | sh"),
        ["echo", "cliclick"],
        "sh runs what it reads"
    );
}

#[test]
fn a_shell_string_filled_in_at_run_time_is_read_for_its_letters() {
    // The kill guard calls such a string unknown; a reader of names reads it.
    assert_eq!(names("bash -c \"cliclick c:$X,$Y\""), ["cliclick"]);
}

#[test]
fn a_name_only_in_an_argument_is_not_a_program() {
    assert_eq!(names("echo cliclick"), ["echo"]);
    assert_eq!(names("grep -rn xdotool ."), ["grep"]);
    assert_eq!(names("echo 'kill 1; cliclick p'"), ["echo"]);
}

#[test]
fn its_arguments_and_input_are_their_letters() {
    let runs = programs_run(
        "osascript -e 'tell application \"Google Chrome\" to activate' <<<\"x\"",
        |_| true,
    );
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].program, "osascript");
    assert_eq!(
        runs[0].args,
        ["-e", "tell application \"Google Chrome\" to activate"]
    );
    assert_eq!(runs[0].input.len(), 1);
}

#[test]
fn text_the_caller_says_no_to_is_not_read() {
    assert!(programs_run("cliclick p", |_| false).is_empty());
    // The outer command is read; the shell's string is asked again.
    let inner_refused = programs_run("bash -c 'cliclick p'", |text| !text.starts_with("cliclick"));
    assert!(inner_refused.is_empty());
}
