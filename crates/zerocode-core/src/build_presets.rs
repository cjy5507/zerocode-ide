//! What the board suggests for a project that has not written its builds'
//! order yet: one build for each build tool its checkout's root shows, read
//! off one data table (`build_presets.toml`). Nothing is run. A preset reads
//! whether its files stand and, for scripts and targets, those files' text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use serde::Deserialize;

use crate::build_file::{BuildFile, BuildSpec, Source, StepSpec, When};
use crate::builds::Text;
use crate::file_stamp::{FileStamp, stamp_plain};

/// A project file this large is not read for its scripts or targets.
const READ_BYTES_MAX: u64 = 256 * 1024;

/// How a makefile or a justfile line that sets a variable goes on after its
/// name: such a line names no target.
const ASSIGNMENTS: [&str; 5] = ["=", "?=", "+=", "!=", ":="];

static PRESETS: LazyLock<Vec<Preset>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Table {
        preset: Vec<Preset>,
    }
    toml::from_str::<Table>(include_str!("build_presets.toml"))
        .map(|table| table.preset)
        .unwrap_or_default()
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    id: String,
    tool: String,
    found: Vec<String>,
    #[serde(default)]
    needs: Option<Needs>,
    runner: Vec<Runner>,
    name: Text,
    step: Vec<PresetStep>,
}

/// Every tool a preset names: its own, and its runners'.
#[cfg(test)]
pub(crate) fn tools() -> Vec<&'static str> {
    PRESETS
        .iter()
        .flat_map(|preset| {
            std::iter::once(preset.tool.as_str()).chain(
                preset
                    .runner
                    .iter()
                    .filter_map(|runner| runner.tool.as_deref()),
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Needs {
    /// A script of the root's `package.json`.
    Script,
    /// A target of the file the preset was found by.
    Target,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Runner {
    #[serde(default)]
    file: Option<String>,
    run: String,
    #[serde(default)]
    windows: Option<String>,
    #[serde(default)]
    tool: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetStep {
    id: String,
    args: String,
    name: Text,
    #[serde(default)]
    when: When,
}

/// The builds the checkout's root suggests, in the table's order: each preset
/// one of whose files stands there, with the steps that can run there. A
/// preset left with no step is not suggested.
#[must_use]
pub fn suggest(checkout: &Path) -> BuildFile {
    BuildFile {
        builds: PRESETS
            .iter()
            .filter_map(|preset| suggested(preset, checkout))
            .collect(),
    }
}

/// Every file name at the root a suggestion reads: what a caller stamps to
/// know the suggestion still holds.
static READ_FILES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut names: Vec<&str> = PRESETS
        .iter()
        .flat_map(|preset| {
            preset
                .found
                .iter()
                .chain(
                    preset
                        .runner
                        .iter()
                        .filter_map(|runner| runner.file.as_ref()),
                )
                .map(String::as_str)
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
});

fn suggested(preset: &Preset, checkout: &Path) -> Option<BuildSpec> {
    let found = preset
        .found
        .iter()
        .map(|name| checkout.join(name))
        .find(|path| stamp_plain(path).is_some())?;
    let runner = preset.runner.iter().find(|runner| {
        runner
            .file
            .as_ref()
            .is_none_or(|file| stamp_plain(&checkout.join(file)).is_some())
    })?;
    let has: Option<HashSet<String>> = match preset.needs {
        None => None,
        Some(Needs::Script) => Some(scripts_of(&checkout.join("package.json"))),
        Some(Needs::Target) => Some(targets_of(&found)),
    };
    let folder = image_name(checkout).unwrap_or_else(|| preset.id.clone());
    let steps: Vec<StepSpec> = preset
        .step
        .iter()
        .filter(|step| has.as_ref().is_none_or(|has| has.contains(&step.args)))
        .map(|step| {
            let args = step.args.replace("{folder}", &folder);
            StepSpec {
                id: step.id.clone(),
                name: Some(step.name.clone()),
                run: Some(format!("{} {args}", runner.run)),
                windows: runner
                    .windows
                    .as_ref()
                    .map(|windows| format!("{windows} {args}")),
                cap: None,
                when: step.when,
            }
        })
        .collect();
    (!steps.is_empty()).then(|| BuildSpec {
        id: preset.id.clone(),
        name: Some(preset.name.clone()),
        tool: Some(runner.tool.clone().unwrap_or_else(|| preset.tool.clone())),
        after: None,
        source: Source::Run,
        steps,
    })
}

fn read_text(path: &Path) -> Option<String> {
    stamp_plain(path).filter(|stamp| stamp.len <= READ_BYTES_MAX)?;
    std::fs::read_to_string(path).ok()
}

/// The scripts a `package.json` names.
fn scripts_of(package: &Path) -> HashSet<String> {
    #[derive(Deserialize)]
    struct Package {
        #[serde(default)]
        scripts: serde_json::Map<String, serde_json::Value>,
    }
    read_text(package)
        .and_then(|text| serde_json::from_str::<Package>(&text).ok())
        .map(|package| package.scripts.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default()
}

/// The targets a makefile or a justfile names: each line that starts with a
/// name and goes on to a `:` that is not `:=` names one (a justfile's recipe
/// may take arguments before it; a quiet recipe leads with `@`).
fn targets_of(file: &Path) -> HashSet<String> {
    read_text(file)
        .map(|text| {
            text.lines()
                .filter_map(target_of)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn target_of(line: &str) -> Option<&str> {
    let line = line.strip_prefix('@').unwrap_or(line);
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .unwrap_or(line.len());
    let (name, rest) = line.split_at(end);
    let assigns = ASSIGNMENTS
        .iter()
        .any(|op| rest.trim_start().starts_with(op));
    let colon = rest.find(':')?;
    (!name.is_empty() && !assigns && !rest[colon + 1..].starts_with('=')).then_some(name)
}

/// The checkout's folder name as an image name: lowercase letters and digits,
/// with `.`, `_` and `-` between them; none when nothing of it is left.
fn image_name(checkout: &Path) -> Option<String> {
    let folder = checkout.file_name()?.to_string_lossy().to_lowercase();
    let mut name = String::with_capacity(folder.len());
    for c in folder.chars() {
        let kept = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if !(kept == '-' && name.ends_with('-')) {
            name.push(kept);
        }
    }
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    (!name.is_empty()).then(|| name.to_owned())
}

/// The window's memory of one checkout's suggestion: it is drafted again only
/// when a file it reads stood up, went away or moved.
#[derive(Debug, Default)]
pub struct Detector {
    held: Option<(PathBuf, Vec<Option<FileStamp>>, Arc<BuildFile>)>,
}

impl Detector {
    /// What `checkout`'s root suggests ([`suggest`]).
    pub fn suggest(&mut self, checkout: &Path) -> Arc<BuildFile> {
        let stamps: Vec<Option<FileStamp>> = READ_FILES
            .iter()
            .map(|name| stamp_plain(&checkout.join(name)))
            .collect();
        if let Some((held_checkout, held_stamps, held)) = &self.held
            && held_checkout == checkout
            && *held_stamps == stamps
        {
            return Arc::clone(held);
        }
        let drafted = Arc::new(suggest(checkout));
        self.held = Some((checkout.to_path_buf(), stamps, Arc::clone(&drafted)));
        drafted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_file::{check, parse, to_toml};

    fn root_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::Builder::new()
            .prefix("Shop API ")
            .tempdir()
            .expect("a checkout");
        for (name, text) in files {
            std::fs::write(root.path().join(name), text).expect("a project file");
        }
        root
    }

    fn ids(file: &BuildFile) -> Vec<&str> {
        file.builds.iter().map(|build| build.id.as_str()).collect()
    }

    fn commands<'a>(file: &'a BuildFile, build: &str) -> Vec<&'a str> {
        file.builds
            .iter()
            .find(|one| one.id == build)
            .map(|one| {
                one.steps
                    .iter()
                    .filter_map(|step| step.run.as_deref())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_table_reads_and_every_preset_drafts_a_file_that_runs_as_written() {
        assert!(!PRESETS.is_empty(), "the presets table did not read");
        let every: Vec<(&str, &str)> = PRESETS
            .iter()
            .flat_map(|preset| {
                preset
                    .found
                    .iter()
                    .chain(preset.runner.iter().filter_map(|one| one.file.as_ref()))
            })
            .map(|name| (name.as_str(), ""))
            .collect();
        let root = root_with(&every);
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts": {"lint": "eslint .", "test": "vitest", "build": "vite build"}}"#,
        )
        .expect("a package");
        for name in ["GNUmakefile", "justfile"] {
            std::fs::write(
                root.path().join(name),
                "lint:\n\techo\ntest:\n\techo\nbuild:\n\techo\n",
            )
            .expect("targets");
        }
        let drafted = suggest(root.path());
        assert_eq!(
            ids(&drafted),
            PRESETS
                .iter()
                .map(|preset| preset.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(check(&drafted), Ok(()));
        assert_eq!(parse(&to_toml(&drafted)), Ok(drafted));
    }

    #[test]
    fn a_java_server_is_suggested_its_wrapper_and_an_image_named_for_its_folder() {
        let root = root_with(&[
            ("gradlew", ""),
            ("build.gradle.kts", ""),
            ("Dockerfile", "FROM scratch\n"),
        ]);
        let drafted = suggest(root.path());
        assert_eq!(ids(&drafted), ["gradle", "image"]);
        assert_eq!(
            commands(&drafted, "gradle"),
            [
                "./gradlew compileJava",
                "./gradlew test",
                "./gradlew assemble"
            ]
        );
        assert_eq!(
            drafted.builds[0].steps[0].windows.as_deref(),
            Some("gradlew.bat compileJava")
        );
        let image = commands(&drafted, "image");
        let folder = image_name(root.path()).expect("an image name");
        assert!(folder.starts_with("shop-api-"), "{folder}");
        assert_eq!(
            image,
            [
                format!("docker build -t {folder}:${{sha8}} ."),
                format!("docker push {folder}:${{sha8}}")
            ]
        );
        assert_eq!(drafted.builds[1].steps[1].when, When::Manual);
    }

    #[test]
    fn scripts_and_targets_are_only_the_ones_the_project_has() {
        let root = root_with(&[
            (
                "package.json",
                r#"{"scripts": {"test": "vitest", "dev": "vite"}}"#,
            ),
            ("yarn.lock", ""),
            (
                "justfile",
                "set shell := [\"bash\"]\nversion := \"1\"\n@lint arg='x':\n  echo\ndeploy:\n  echo\n",
            ),
            ("Makefile", "CC=cc\nbuild: main.o\n\tcc\n.PHONY: build\n"),
        ]);
        let drafted = suggest(root.path());
        assert_eq!(ids(&drafted), ["node", "make", "just"]);
        assert_eq!(commands(&drafted, "node"), ["yarn run test"]);
        assert_eq!(drafted.builds[0].tool.as_deref(), Some("yarn"));
        assert_eq!(commands(&drafted, "make"), ["make build"]);
        assert_eq!(commands(&drafted, "just"), ["just lint"]);
    }

    #[test]
    fn a_root_with_no_build_tool_or_none_of_its_steps_suggests_nothing() {
        let bare = root_with(&[("README.md", "# hi\n")]);
        assert!(suggest(bare.path()).builds.is_empty());
        let scriptless = root_with(&[
            ("package.json", r#"{"name": "x"}"#),
            ("Makefile", "all:\n\techo\n"),
        ]);
        assert!(suggest(scriptless.path()).builds.is_empty());
    }

    #[test]
    fn a_suggestion_is_drafted_again_only_when_a_file_it_reads_moved() {
        let root = root_with(&[("Cargo.toml", "[package]\n")]);
        let mut detector = Detector::default();
        let first = detector.suggest(root.path());
        assert!(
            Arc::ptr_eq(&first, &detector.suggest(root.path())),
            "drafted again with nothing moved"
        );
        std::fs::write(root.path().join("go.mod"), "module x\n").expect("a module");
        let moved = detector.suggest(root.path());
        assert_eq!(ids(&moved), ["cargo", "go"]);
    }

    #[test]
    fn an_image_name_is_the_folders_in_lowercase_with_its_odd_letters_joined() {
        assert_eq!(
            image_name(Path::new("/work/Shop API (v2)")),
            Some("shop-api-v2".into())
        );
        assert_eq!(image_name(Path::new("/work/한국어")), None);
        assert_eq!(target_of("test: build"), Some("test"));
        assert_eq!(target_of("x := 1"), None);
        assert_eq!(target_of("CC=cc"), None);
        assert_eq!(target_of("\techo"), None);
        assert_eq!(target_of("@lint arg='x':"), Some("lint"));
        assert_eq!(target_of("set shell := [\"bash\"]"), None);
    }
}
