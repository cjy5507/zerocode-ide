//! What the board suggests for a project that has not written its builds'
//! order yet: one build for each build tool its checkout shows — at the root,
//! and in the subfolders a monorepo keeps its projects in — read off one data
//! table (`build_presets.toml`). Nothing is run. A preset reads whether its
//! files stand and, for scripts, targets and words, those files' text.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use serde::Deserialize;

use crate::build_file::{BuildFile, BuildSpec, Source, StepSpec, When};
use crate::builds::Text;
use crate::file_stamp::{FileStamp, stamp_of, stamp_plain};

/// A project file this large is not read for its scripts, targets or words.
const READ_BYTES_MAX: u64 = 256 * 1024;

/// How a makefile or a justfile line that sets a variable goes on after its
/// name: such a line names no target.
const ASSIGNMENTS: [&str; 5] = ["=", "?=", "+=", "!=", ":="];

/// A `found` name that stands for any file of an extension (`*.sln`).
const ANY_NAMED: &str = "*.";

#[derive(Debug, Deserialize)]
struct Table {
    scan: Scan,
    preset: Vec<Preset>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    toml::from_str::<Table>(include_str!("build_presets.toml")).unwrap_or(Table {
        scan: Scan::default(),
        preset: Vec::new(),
    })
});

/// How far under the root the suggestion looks ([`TABLE`]'s `[scan]`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scan {
    /// The deepest subfolder looked into: 1 is the root's own folders.
    deepest: usize,
    /// The most builds suggested.
    most: usize,
    /// Folder names never looked into (a folder whose name starts with `.` is
    /// not either).
    skip: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    id: String,
    tool: String,
    found: Vec<String>,
    #[serde(default)]
    needs: Option<Needs>,
    #[serde(default)]
    when_text: Option<Words>,
    #[serde(default)]
    unless_text: Option<Words>,
    runner: Vec<Runner>,
    name: Text,
    step: Vec<PresetStep>,
}

/// Words one of a folder's files may hold.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Words {
    files: Vec<String>,
    any: Vec<String>,
}

/// Every tool a preset names: its own, and its runners'.
#[cfg(test)]
pub(crate) fn tools() -> Vec<&'static str> {
    TABLE
        .preset
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
    /// A script of the file the preset was found by (`package.json`,
    /// `composer.json`), named as the step's `args`.
    Script,
    /// A target of the file the preset was found by, named as the step's `args`.
    Target,
    /// One of the step's `mentions` in the text of the file the preset was found by.
    Mention,
    /// One of the step's `files` in the folder.
    File,
    /// Nothing: the step always stands.
    Nothing,
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
    #[serde(default)]
    args: String,
    /// The whole command, when the step's program is not the preset's runner.
    #[serde(default)]
    run: Option<String>,
    name: Text,
    #[serde(default)]
    when: When,
    #[serde(default)]
    needs: Option<Needs>,
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
}

/// The builds the checkout suggests, in the table's order: each preset one of
/// whose files stands at the root, with the steps that can run there; then the
/// projects in the subfolders `[scan]` reaches, each a build that runs in its
/// folder (`dir`). A preset left with no step is not suggested.
#[must_use]
pub fn suggest(checkout: &Path) -> BuildFile {
    drafted(checkout).0
}

/// The suggestion and every path it read: what a caller stamps to know the
/// suggestion still holds.
fn drafted(checkout: &Path) -> (BuildFile, Vec<PathBuf>) {
    let mut read = vec![checkout.to_path_buf()];
    let mut builds: Vec<BuildSpec> = Vec::new();
    let mut root_tools: HashSet<String> = HashSet::new();
    for preset in &TABLE.preset {
        if let Some(build) = suggested(preset, checkout, None, &mut read) {
            root_tools.insert(preset.tool.clone());
            builds.push(build);
        }
    }
    // Breadth first, in name order: the root's own folders, then theirs.
    let mut folders = VecDeque::from([(PathBuf::new(), 0usize)]);
    while let Some((under, depth)) = folders.pop_front() {
        if depth >= TABLE.scan.deepest || builds.len() >= TABLE.scan.most {
            continue;
        }
        let mut children: Vec<PathBuf> = subfolders(&checkout.join(&under))
            .into_iter()
            .map(|name| under.join(name))
            .collect();
        children.sort();
        for dir in children {
            let folder = checkout.join(&dir);
            read.push(folder.clone());
            let found: Vec<BuildSpec> = TABLE
                .preset
                .iter()
                .filter(|preset| !root_tools.contains(&preset.tool))
                .filter_map(|preset| suggested(preset, &folder, Some(dir.as_path()), &mut read))
                .collect();
            if found.is_empty() {
                folders.push_back((dir, depth + 1));
            } else {
                builds.extend(found);
            }
        }
    }
    builds.truncate(TABLE.scan.most.max(1));
    read.sort();
    read.dedup();
    (BuildFile { builds }, read)
}

/// The folders under `folder` a suggestion may look into.
fn subfolders(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.') && !TABLE.scan.skip.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// The first file of `names` that stands in `folder` (a `*.ext` name is any
/// file of that extension there).
fn found_in(folder: &Path, names: &[String], read: &mut Vec<PathBuf>) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        if let Some(extension) = name.strip_prefix(ANY_NAMED) {
            let mut matching: Vec<PathBuf> = std::fs::read_dir(folder)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == extension) && path.is_file()
                })
                .collect();
            matching.sort();
            let first = matching.into_iter().next()?;
            read.push(first.clone());
            return Some(first);
        }
        let path = folder.join(name);
        read.push(path.clone());
        stamp_plain(&path).is_some().then_some(path)
    })
}

/// Whether one of `words.files` in `folder` holds one of `words.any`.
fn holds(folder: &Path, words: &Words, read: &mut Vec<PathBuf>) -> bool {
    words.files.iter().any(|name| {
        let path = folder.join(name);
        read.push(path.clone());
        read_text(&path)
            .is_some_and(|text| words.any.iter().any(|word| text.contains(word.as_str())))
    })
}

fn suggested(
    preset: &Preset,
    folder: &Path,
    dir: Option<&Path>,
    read: &mut Vec<PathBuf>,
) -> Option<BuildSpec> {
    let found = found_in(folder, &preset.found, read)?;
    if preset
        .when_text
        .as_ref()
        .is_some_and(|words| !holds(folder, words, read))
        || preset
            .unless_text
            .as_ref()
            .is_some_and(|words| holds(folder, words, read))
    {
        return None;
    }
    let runner = preset.runner.iter().find(|runner| {
        runner.file.as_ref().is_none_or(|file| {
            let path = folder.join(file);
            read.push(path.clone());
            stamp_plain(&path).is_some()
        })
    })?;
    let text = read_text(&found).unwrap_or_default();
    let scripts = scripts_of(&text);
    let targets = targets_of(&text);
    let folder_name = image_name(folder).unwrap_or_else(|| preset.id.clone());
    let steps: Vec<StepSpec> = preset
        .step
        .iter()
        .filter(|step| match step.needs.or(preset.needs) {
            None | Some(Needs::Nothing) => true,
            Some(Needs::Script) => scripts.contains(&step.args),
            Some(Needs::Target) => targets.contains(&step.args),
            Some(Needs::Mention) => step
                .mentions
                .iter()
                .any(|word| text.contains(word.as_str())),
            Some(Needs::File) => step.files.iter().any(|name| {
                let path = folder.join(name);
                read.push(path.clone());
                stamp_plain(&path).is_some()
            }),
        })
        .map(|step| {
            let args = step.args.replace("{folder}", &folder_name);
            let (run, windows) = match &step.run {
                Some(own) => (own.clone(), None),
                None => (
                    format!("{} {args}", runner.run),
                    runner
                        .windows
                        .as_ref()
                        .map(|windows| format!("{windows} {args}")),
                ),
            };
            StepSpec {
                id: step.id.clone(),
                name: Some(step.name.clone()),
                run: Some(run),
                windows,
                cap: None,
                when: step.when,
            }
        })
        .collect();
    if steps.is_empty() {
        return None;
    }
    let dir = dir.map(|dir| dir.to_string_lossy().replace('\\', "/"));
    Some(BuildSpec {
        id: dir.as_deref().map_or_else(
            || preset.id.clone(),
            |dir| format!("{}-{}", build_slug(dir), preset.id),
        ),
        name: Some(match dir.as_deref() {
            Some(dir) => in_folder(&preset.name, dir),
            None => preset.name.clone(),
        }),
        tool: Some(runner.tool.clone().unwrap_or_else(|| preset.tool.clone())),
        after: None,
        dir,
        source: Source::Run,
        steps,
        retry: None,
        stop: None,
    })
}

/// A preset's name in every language, led by the folder it runs in.
fn in_folder(name: &Text, dir: &str) -> Text {
    match name {
        Text::One(one) => Text::One(format!("{dir} · {one}")),
        Text::ByLanguage(by) => Text::ByLanguage(
            by.iter()
                .map(|(language, words)| (language.clone(), format!("{dir} · {words}")))
                .collect(),
        ),
    }
}

/// A folder as the head of a build id: its letters and digits, other runs one `-`.
fn build_slug(dir: &str) -> String {
    let mut slug = String::with_capacity(dir.len());
    for c in dir.chars() {
        let kept = if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            '-'
        };
        if !(kept == '-' && (slug.is_empty() || slug.ends_with('-'))) {
            slug.push(kept);
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "sub".to_owned()
    } else {
        slug.to_owned()
    }
}

fn read_text(path: &Path) -> Option<String> {
    stamp_plain(path).filter(|stamp| stamp.len <= READ_BYTES_MAX)?;
    std::fs::read_to_string(path).ok()
}

/// The scripts a JSON manifest names (`package.json`, `composer.json`).
fn scripts_of(text: &str) -> HashSet<String> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(default)]
        scripts: serde_json::Map<String, serde_json::Value>,
    }
    serde_json::from_str::<Manifest>(text)
        .map(|manifest| manifest.scripts.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default()
}

/// The targets a makefile or a justfile names: each line that starts with a
/// name and goes on to a `:` that is not `:=` names one (a justfile's recipe
/// may take arguments before it; a quiet recipe leads with `@`).
fn targets_of(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(target_of)
        .map(str::to_owned)
        .collect()
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

/// The folder's name as an image name: lowercase letters and digits, with `.`,
/// `_` and `-` between them; none when nothing of it is left.
fn image_name(folder: &Path) -> Option<String> {
    let folder = folder.file_name()?.to_string_lossy().to_lowercase();
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
/// when a file or a folder it read stood up, went away or moved.
#[derive(Debug, Default)]
pub struct Detector {
    held: Option<Held>,
}

/// A drafted suggestion: the checkout it is of, each path it read with that
/// path's stamp then, and the draft.
type Held = (PathBuf, Vec<(PathBuf, Option<FileStamp>)>, Arc<BuildFile>);

impl Detector {
    /// What `checkout` suggests ([`suggest`]).
    pub fn suggest(&mut self, checkout: &Path) -> Arc<BuildFile> {
        if let Some((held_checkout, held_stamps, held)) = &self.held
            && held_checkout == checkout
            && held_stamps
                .iter()
                .all(|(path, stamp)| stamp_any(path) == *stamp)
        {
            return Arc::clone(held);
        }
        let (file, read) = drafted(checkout);
        let drafted = Arc::new(file);
        let stamps = read.into_iter().map(|path| {
            let stamp = stamp_any(&path);
            (path, stamp)
        });
        self.held = Some((
            checkout.to_path_buf(),
            stamps.collect(),
            Arc::clone(&drafted),
        ));
        drafted
    }
}

/// A plain file's stamp, or a folder's — its entries changing moves its own
/// time. A link is not followed, as [`stamp_plain`] does not.
fn stamp_any(path: &Path) -> Option<FileStamp> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_file() || meta.is_dir())
        .map(|meta| stamp_of(&meta))
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

    /// A root that holds what `preset` needs to stand with every one of its
    /// steps: its first file (or one of its extension), the words it is kept to,
    /// and each step's script, target, mention or file.
    fn root_for(preset: &Preset) -> tempfile::TempDir {
        let root = root_with(&[]);
        let write = |name: &str, text: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
            std::fs::write(path, text).expect("a project file");
        };
        let first = preset.found[0].replace("*.", "Shop.");
        let mut text = String::new();
        if let Some(words) = &preset.when_text {
            text.push_str(&words.any[0]);
            text.push('\n');
        }
        let scripts: Vec<String> = preset
            .step
            .iter()
            .map(|step| format!("\"{}\": \"x\"", step.args))
            .collect();
        for step in &preset.step {
            match step.needs.or(preset.needs) {
                Some(Needs::Target) => {
                    text.push_str(&step.args);
                    text.push_str(":\n\techo\n");
                }
                Some(Needs::Mention) => {
                    text.push_str(&step.mentions[0]);
                    text.push('\n');
                }
                Some(Needs::File) => write(&step.files[0], ""),
                _ => {}
            }
        }
        if matches!(preset.needs, Some(Needs::Script)) {
            text = format!("{{\"scripts\": {{{}}}}}", scripts.join(", "));
        }
        write(&first, &text);
        if let Some(words) = &preset.when_text
            && words.files[0] != first
        {
            write(&words.files[0], &words.any[0]);
        }
        root
    }

    #[test]
    fn the_table_reads_and_every_preset_drafts_every_step_in_a_file_that_runs_as_written() {
        assert!(
            TABLE.preset.len() >= 19,
            "the presets table did not read: {}",
            TABLE.preset.len()
        );
        assert!(
            TABLE.scan.deepest >= 1 && TABLE.scan.most >= 1,
            "the scan table did not read"
        );
        for preset in &TABLE.preset {
            let root = root_for(preset);
            let drafted = suggest(root.path());
            let build = drafted
                .builds
                .iter()
                .find(|build| build.id == preset.id)
                .unwrap_or_else(|| {
                    panic!(
                        "`{}` is not suggested for its own files: {:?}",
                        preset.id,
                        ids(&drafted)
                    )
                });
            assert_eq!(
                build.steps.len(),
                preset.step.len(),
                "`{}` dropped a step its root has",
                preset.id
            );
            assert_eq!(check(&drafted), Ok(()), "`{}`", preset.id);
            assert_eq!(
                parse(&to_toml(&drafted)),
                Ok(drafted.clone()),
                "`{}`",
                preset.id
            );
        }
    }

    #[test]
    fn an_android_or_multiplatform_gradle_project_is_suggested_its_own_tasks_never_compile_java() {
        let android = root_with(&[
            ("gradlew", ""),
            ("settings.gradle.kts", "include(\":app\")\n"),
        ]);
        std::fs::create_dir_all(android.path().join("app")).expect("the app");
        std::fs::write(
            android.path().join("app/build.gradle.kts"),
            "plugins { id(\"com.android.application\") }\n",
        )
        .expect("the app's build");
        let drafted = suggest(android.path());
        assert_eq!(ids(&drafted), ["android"]);
        assert_eq!(
            commands(&drafted, "android"),
            [
                "./gradlew assembleDebug",
                "./gradlew testDebugUnitTest",
                "./gradlew lintDebug"
            ]
        );
        let kmp = root_with(&[
            ("gradlew", ""),
            (
                "build.gradle.kts",
                "plugins { kotlin(\"multiplatform\") }\n",
            ),
        ]);
        assert_eq!(ids(&suggest(kmp.path())), ["kmp"]);
        let flutter = root_with(&[(
            "pubspec.yaml",
            "dependencies:\n  flutter:\n    sdk: flutter\n",
        )]);
        assert_eq!(ids(&suggest(flutter.path())), ["flutter"]);
        let dart = root_with(&[("pubspec.yaml", "name: shop\n")]);
        assert_eq!(ids(&suggest(dart.path())), ["dart"]);
    }

    #[test]
    fn a_python_project_is_suggested_the_tools_it_names_through_its_own_environment() {
        let root = root_with(&[
            (
                "pyproject.toml",
                "[tool.ruff]\nline-length = 100\n[dependency-groups]\ndev = [\"pytest>=8\"]\n",
            ),
            ("uv.lock", ""),
        ]);
        let drafted = suggest(root.path());
        assert_eq!(ids(&drafted), ["python"]);
        assert_eq!(
            commands(&drafted, "python"),
            ["uv run python -m ruff check .", "uv run python -m pytest"],
            "mypy is not named and no build system is declared"
        );
        let dotnet = root_with(&[("Shop.sln", "")]);
        assert_eq!(
            commands(&suggest(dotnet.path()), "dotnet"),
            [
                "dotnet restore",
                "dotnet build --no-restore",
                "dotnet test --no-build",
                "dotnet publish -c Release"
            ]
        );
        let go = root_with(&[("go.mod", "module shop\n"), (".golangci.yml", "")]);
        assert_eq!(
            commands(&suggest(go.path()), "go"),
            [
                "go vet ./...",
                "golangci-lint run ./...",
                "go test ./...",
                "go build ./..."
            ]
        );
    }

    #[test]
    fn a_monorepo_is_suggested_a_build_per_project_in_its_folders_each_run_where_it_stands() {
        let root = root_with(&[("README.md", "# shop\n")]);
        let write = |name: &str, text: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
            std::fs::write(path, text).expect("a project file");
        };
        write("backend/go.mod", "module shop\n");
        write(
            "web/package.json",
            r#"{"scripts": {"test": "vitest", "build": "vite build"}}"#,
        );
        write("web/pnpm-lock.yaml", "");
        write("services/billing/pom.xml", "<project/>\n");
        write(
            "web/node_modules/left-pad/package.json",
            r#"{"scripts": {"test": "x"}}"#,
        );
        write(".cache/tool/Cargo.toml", "[package]\n");
        write("backend/tools/gen/go.mod", "module gen\n");
        let drafted = suggest(root.path());
        assert_eq!(
            ids(&drafted),
            ["backend-go", "web-node", "services-billing-maven"]
        );
        let go = &drafted.builds[0];
        assert_eq!(go.dir.as_deref(), Some("backend"));
        assert_eq!(go.folder(root.path()), root.path().join("backend"));
        assert_eq!(
            go.name.as_ref().and_then(|name| match name {
                Text::ByLanguage(by) => by.get("ko").cloned(),
                Text::One(one) => Some(one.clone()),
            }),
            Some("backend · Go 빌드".to_owned())
        );
        assert_eq!(
            commands(&drafted, "web-node"),
            ["pnpm run test", "pnpm run build"]
        );
        assert_eq!(drafted.builds[2].dir.as_deref(), Some("services/billing"));
        assert_eq!(check(&drafted), Ok(()));
        assert_eq!(parse(&to_toml(&drafted)), Ok(drafted.clone()));

        let workspace = root_with(&[(
            "package.json",
            r#"{"workspaces": ["apps/*"], "scripts": {"build": "turbo build"}}"#,
        )]);
        std::fs::create_dir_all(workspace.path().join("apps/web")).expect("an app");
        std::fs::write(
            workspace.path().join("apps/web/package.json"),
            r#"{"scripts": {"build": "x"}}"#,
        )
        .expect("its package");
        assert_eq!(
            ids(&suggest(workspace.path())),
            ["node"],
            "a workspace's packages are the root's own build"
        );
    }

    #[test]
    fn a_monorepo_suggestion_is_drafted_again_when_a_folder_gains_a_project() {
        let root = root_with(&[("README.md", "# shop\n")]);
        std::fs::create_dir_all(root.path().join("backend")).expect("a folder");
        let mut detector = Detector::default();
        let empty = detector.suggest(root.path());
        assert!(empty.builds.is_empty());
        assert!(
            Arc::ptr_eq(&empty, &detector.suggest(root.path())),
            "drafted again with nothing moved"
        );
        std::fs::write(root.path().join("backend/go.mod"), "module shop\n").expect("a module");
        assert_eq!(ids(&detector.suggest(root.path())), ["backend-go"]);
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
