//! Whether a shell command is a check — a build, a test, a run whose exit says
//! something about the work (moved here from zo's deep lane by t-11349, so a
//! completion claim is read by the same rule in zo and in the window's panes,
//! `crate::jev::claim`).

/// Command substrings that name a project's own build, lint or test gate,
/// aligned with zo's `detect_check_command`'s per-ecosystem vocabulary: a green
/// run of one verifies a change wherever it is typed.
pub const PROJECT_CHECK_MARKERS: &[&str] = &[
    "cargo build",
    "cargo check",
    "cargo clippy",
    "cargo test",
    "npm test",
    "yarn test",
    "pnpm test",
    "pytest",
    "go build",
    "go test",
    "go vet",
    "dotnet build",
    "dotnet test",
    "make test",
    "make check",
    "make build",
    "tsc",
    "./gradlew",
    "mvn test",
];

/// The direct runs implementers type to check their own output (`python3
/// test_x.py`, `cargo run` as an output check). Their green exit is worth
/// reporting to the verifier, and names no project gate: `python3 -m
/// json.tool` and `npm run dev` are shaped the same.
const RUN_CHECK_MARKERS: &[&str] = &[
    "cargo run",
    "npm run",
    "python -m",
    "python3 -m",
    "python test",
    "python3 test",
    "python tests",
    "python3 tests",
    "go run",
];

/// Whether `command` is shaped like a check: it names one of
/// [`PROJECT_CHECK_MARKERS`] or one of the direct runs implementers type
/// (`RUN_CHECK_MARKERS`), or it is an interpreter heredoc that asserts something.
#[must_use]
pub fn command_is_check_shaped(command: &str) -> bool {
    PROJECT_CHECK_MARKERS
        .iter()
        .chain(RUN_CHECK_MARKERS)
        .any(|marker| command.contains(marker))
        || command_is_assert_heredoc(command)
}

/// The project gates `command` runs, as [`PROJECT_CHECK_MARKERS`] spell them —
/// what a green run of it verified about the project, beyond this one run.
///
/// Each gate must stand as words of its own: `cargo test` holds `go test`, and
/// `tsconfig.json` holds `tsc`, neither of which ran.
pub fn project_checks_in(command: &str) -> impl Iterator<Item = &'static str> + '_ {
    let joins = |c: char| c.is_alphanumeric() || c == '_';
    PROJECT_CHECK_MARKERS.iter().copied().filter(move |marker| {
        command.match_indices(marker).any(|(at, _)| {
            !command[..at].chars().next_back().is_some_and(joins)
                && !command[at + marker.len()..]
                    .chars()
                    .next()
                    .is_some_and(joins)
        })
    })
}

/// The other shape a self-verification takes: an inline interpreter heredoc
/// whose body asserts something (`python3 - <<'EOF' … assert … EOF`).
///
/// Measured on the deep lanes this is the DOMINANT check shape — the model
/// writes an ad-hoc assert script per stage rather than a `pytest` run — and
/// the marker list above cannot name it because the command line carries no
/// tool name, just `python3 -`. Without this arm the verified-state ledger
/// records nothing in exactly the sessions it was built for (observed live:
/// ten turns, an empty ledger, every injection declined `no_green_checks`).
///
/// The `assert`/`raise` requirement keeps data-transform heredocs (scripts
/// that only compute or write) out: a green transform proves nothing worth
/// carrying, and recording one would let a tree-mutating script masquerade as
/// a still-true check. A script that both asserts and writes files can still
/// slip in — a known limit, mitigated by the observation's own wording (the
/// harness never claims coverage, it lists edits and lets the model judge).
fn command_is_assert_heredoc(command: &str) -> bool {
    let Some(heredoc) = command.find("<<") else {
        return false;
    };
    let opener = &command[..heredoc];
    let opener_is_interpreter = ["python3", "python"].iter().any(|interpreter| {
        opener
            .split_whitespace()
            .any(|word| word == *interpreter || word.ends_with(&format!("/{interpreter}")))
    });
    if !opener_is_interpreter {
        return false;
    }
    let body = &command[heredoc..];
    body.contains("assert") || body.contains("raise ")
}
