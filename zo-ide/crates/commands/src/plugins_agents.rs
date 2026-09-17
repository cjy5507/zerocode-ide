use std::collections::BTreeMap;
use std::env;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use plugins::{PluginError, PluginManager, PluginSummary};
use runtime::mcp_oauth::{authenticate_mcp_server_remote, LocalBrowserOpener, McpAuthResult};
use runtime::{
    ConfigLoader, ConfigSource, McpOAuthConfig, McpServerConfig, PermissionMode, RuntimeConfig,
    ScopedMcpServerConfig, Session, UntrustedMcpServer,
};

use crate::mcp_command::McpAction;
use crate::slash_commands::normalize_optional_args;

const MIN_REPORT_LABEL_FIELD: usize = 17;
const MCP_SERVER_REPORT_LABELS: &[&str] = &[
    "Working directory",
    "Result",
    "Name",
    "Scope",
    "Transport",
    "Command",
    "Args",
    "Env keys",
    "Tool timeout",
    "URL",
    "Header keys",
    "Header helper",
    "OAuth",
    "SDK name",
    "Proxy id",
];

fn report_field_width(labels: &[&str]) -> usize {
    debug_assert!(labels.iter().all(|label| label.is_ascii()));
    labels
        .iter()
        .map(|label| label.len() + 1)
        .max()
        .unwrap_or(MIN_REPORT_LABEL_FIELD)
        .max(MIN_REPORT_LABEL_FIELD)
}

fn report_row(field_width: usize, label: &str, value: impl Display) -> String {
    format!("  {label:<field_width$}{value}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandResult {
    pub message: String,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsCommandResult {
    pub message: String,
    pub reload_runtime: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DefinitionSource {
    ProjectZo,
    UserZoConfigHome,
    UserZoHome,
    UserZo,
}

impl DefinitionSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ProjectZo => "Project (.zo)",
            Self::UserZoConfigHome => "User ($ZO_CONFIG_HOME)",
            Self::UserZoHome => "User ($ZO_HOME)",
            Self::UserZo => "User (~/.zo)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSummary {
    name: String,
    description: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    source: DefinitionSource,
    shadowed_by: Option<DefinitionSource>,
}

const NATIVE_AGENT_BUILTIN_TYPES: &[&str] = &[
    "general-purpose",
    "Explore",
    "Plan",
    "Verification",
    "deep-research",
    "code-reviewer",
    "debugger",
    "data-analyst",
    "refactor",
    "zo-guide",
    "statusline-setup",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillSummary {
    name: String,
    description: Option<String>,
    source: DefinitionSource,
    shadowed_by: Option<DefinitionSource>,
    origin: SkillOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillOrigin {
    SkillsDir,
    LegacyCommandsDir,
}

impl SkillOrigin {
    fn detail_label(self) -> Option<&'static str> {
        match self {
            Self::SkillsDir => None,
            Self::LegacyCommandsDir => Some("legacy /commands"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillRoot {
    pub(crate) source: DefinitionSource,
    pub(crate) path: PathBuf,
    pub(crate) origin: SkillOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledSkill {
    pub(crate) invocation_name: String,
    pub(crate) display_name: Option<String>,
    pub(crate) source: PathBuf,
    pub(crate) registry_root: PathBuf,
    pub(crate) installed_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkillInstallSource {
    Directory { root: PathBuf, prompt_path: PathBuf },
    MarkdownFile { path: PathBuf },
}

#[allow(clippy::too_many_lines)] // flat /plugins action dispatch, mirrors sibling command handlers
pub fn handle_plugins_slash_command(
    action: Option<&str>,
    target: Option<&str>,
    manager: &mut PluginManager,
) -> Result<PluginsCommandResult, PluginError> {
    match action {
        None | Some("list") => Ok(PluginsCommandResult {
            message: render_plugins_report(&manager.list_installed_plugins()?),
            reload_runtime: false,
        }),
        Some("install") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins install <path>".to_string(),
                    reload_runtime: false,
                });
            };
            let install = manager.install(target)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == install.plugin_id);
            Ok(PluginsCommandResult {
                message: render_plugin_install_report(&install.plugin_id, plugin.as_ref()),
                reload_runtime: true,
            })
        }
        Some("enable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins enable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.enable(&plugin.metadata.id)?;
            let field_width = report_field_width(&["Result", "Name", "Version", "Status"]);
            Ok(PluginsCommandResult {
                message: [
                    "Plugins".to_string(),
                    report_row(
                        field_width,
                        "Result",
                        format!("enabled {}", plugin.metadata.id),
                    ),
                    report_row(field_width, "Name", &plugin.metadata.name),
                    report_row(field_width, "Version", &plugin.metadata.version),
                    report_row(field_width, "Status", "enabled"),
                ]
                .join("\n"),
                reload_runtime: true,
            })
        }
        Some("disable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins disable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.disable(&plugin.metadata.id)?;
            let field_width = report_field_width(&["Result", "Name", "Version", "Status"]);
            Ok(PluginsCommandResult {
                message: [
                    "Plugins".to_string(),
                    report_row(
                        field_width,
                        "Result",
                        format!("disabled {}", plugin.metadata.id),
                    ),
                    report_row(field_width, "Name", &plugin.metadata.name),
                    report_row(field_width, "Version", &plugin.metadata.version),
                    report_row(field_width, "Status", "disabled"),
                ]
                .join("\n"),
                reload_runtime: true,
            })
        }
        Some("uninstall") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins uninstall <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            manager.uninstall(target)?;
            let field_width = report_field_width(&["Result"]);
            Ok(PluginsCommandResult {
                message: [
                    "Plugins".to_string(),
                    report_row(field_width, "Result", format!("uninstalled {target}")),
                ]
                .join("\n"),
                reload_runtime: true,
            })
        }
        Some("update") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins update <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            let update = manager.update(target)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == update.plugin_id);
            let name = plugin
                .as_ref()
                .map_or_else(|| update.plugin_id.clone(), |plugin| plugin.metadata.name.clone());
            let status = plugin.as_ref().map_or("unknown", |plugin| {
                if plugin.enabled { "enabled" } else { "disabled" }
            });
            let field_width =
                report_field_width(&["Result", "Name", "Old version", "New version", "Status"]);
            Ok(PluginsCommandResult {
                message: [
                    "Plugins".to_string(),
                    report_row(
                        field_width,
                        "Result",
                        format!("updated {}", update.plugin_id),
                    ),
                    report_row(field_width, "Name", name),
                    report_row(field_width, "Old version", update.old_version),
                    report_row(field_width, "New version", update.new_version),
                    report_row(field_width, "Status", status),
                ]
                .join("\n"),
                reload_runtime: true,
            })
        }
        Some(other) => Ok(PluginsCommandResult {
            message: format!(
                "Unknown /plugins action '{other}'. Use list, install, enable, disable, uninstall, or update."
            ),
            reload_runtime: false,
        }),
    }
}

pub fn handle_agents_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_definition_roots(cwd, "agents");
            let agents = load_agents_from_roots(&roots)?;
            Ok(render_agents_report(&agents))
        }
        Some("-h" | "--help" | "help") => Ok(render_agents_usage(None)),
        Some(args) => Ok(render_agents_usage(Some(args))),
    }
}

pub fn handle_mcp_slash_command(
    args: Option<&str>,
    cwd: &Path,
) -> Result<String, runtime::ConfigError> {
    let loader = ConfigLoader::default_for(cwd);
    render_mcp_report_for(&loader, cwd, args)
}

pub fn handle_skills_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_skill_roots(cwd);
            let skills = load_skills_from_roots(&roots)?;
            Ok(render_skills_report(&skills))
        }
        Some("install") => Ok(render_skills_usage(Some("install"))),
        Some(args) if args.starts_with("install ") => {
            let target = args["install ".len()..].trim();
            if target.is_empty() {
                return Ok(render_skills_usage(Some("install")));
            }
            let install = install_skill(target, cwd)?;
            Ok(render_skill_install_report(&install))
        }
        Some("-h" | "--help" | "help") => Ok(render_skills_usage(None)),
        Some(args) => Ok(render_skills_usage(Some(args))),
    }
}

pub(crate) fn render_mcp_report_for(
    loader: &ConfigLoader,
    cwd: &Path,
    args: Option<&str>,
) -> Result<String, runtime::ConfigError> {
    let normalized = normalize_optional_args(args);
    let tokens: Vec<&str> = normalized
        .map(|value| value.split_whitespace().collect())
        .unwrap_or_default();
    // Match the lenient direct-CLI surface: unparsable input renders usage.
    let Ok(action) = McpAction::parse(&tokens) else {
        return Ok(render_mcp_usage(normalized));
    };
    match action {
        McpAction::List => {
            let config = loader.load()?;
            let mut report = render_mcp_summary_report(cwd, config.mcp().servers());
            if let Some(blocked) =
                render_blocked_mcp_section(&config.mcp().untrusted_project_servers())
            {
                report.push_str("\n\n");
                report.push_str(&blocked);
            }
            Ok(report)
        }
        McpAction::Show(server) => {
            let config = loader.load()?;
            Ok(render_mcp_server_report(
                cwd,
                &server,
                config.mcp().get(&server),
            ))
        }
        McpAction::Help => Ok(render_mcp_usage(None)),
        McpAction::AuthList => {
            let config = loader.load()?;
            Ok(render_mcp_auth_list(cwd, config.mcp().servers()))
        }
        McpAction::Auth(server) => {
            let config = loader.load()?;
            Ok(render_mcp_auth(cwd, &server, &config))
        }
        McpAction::Logout(server) => Ok(render_mcp_logout(cwd, &server)),
    }
}

#[must_use]
pub fn render_plugins_report(plugins: &[PluginSummary]) -> String {
    let mut lines = vec!["Plugins".to_string()];
    if plugins.is_empty() {
        lines.push("  No plugins installed.".to_string());
        return lines.join("\n");
    }
    for plugin in plugins {
        let enabled = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        lines.push(format!(
            "  {name:<20} v{version:<10} {enabled}",
            name = plugin.metadata.name,
            version = plugin.metadata.version,
        ));
    }
    lines.join("\n")
}

fn render_plugin_install_report(plugin_id: &str, plugin: Option<&PluginSummary>) -> String {
    let name = plugin.map_or(plugin_id, |plugin| plugin.metadata.name.as_str());
    let version = plugin.map_or("unknown", |plugin| plugin.metadata.version.as_str());
    let enabled = plugin.is_some_and(|plugin| plugin.enabled);
    let field_width = report_field_width(&["Result", "Name", "Version", "Status"]);
    [
        "Plugins".to_string(),
        report_row(field_width, "Result", format!("installed {plugin_id}")),
        report_row(field_width, "Name", name),
        report_row(field_width, "Version", version),
        report_row(
            field_width,
            "Status",
            if enabled { "enabled" } else { "disabled" },
        ),
    ]
    .join("\n")
}

fn resolve_plugin_target(
    manager: &PluginManager,
    target: &str,
) -> Result<PluginSummary, PluginError> {
    let mut matches = manager
        .list_installed_plugins()?
        .into_iter()
        .filter(|plugin| plugin.metadata.id == target || plugin.metadata.name == target)
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(PluginError::NotFound(format!(
            "plugin `{target}` is not installed or discoverable"
        ))),
        _ => Err(PluginError::InvalidManifest(format!(
            "plugin name `{target}` is ambiguous; use the full plugin id"
        ))),
    }
}

pub(crate) fn discover_definition_roots(
    cwd: &Path,
    leaf: &str,
) -> Vec<(DefinitionSource, PathBuf)> {
    let mut roots = Vec::new();

    for ancestor in cwd.ancestors() {
        push_unique_root(
            &mut roots,
            DefinitionSource::ProjectZo,
            ancestor.join(".zo").join(leaf),
        );
    }

    if let Some(zo_config_home) = env::var_os("ZO_CONFIG_HOME") {
        let zo_config_home = PathBuf::from(zo_config_home);
        if !zo_config_home.as_os_str().is_empty() {
            push_unique_root(
                &mut roots,
                DefinitionSource::UserZoConfigHome,
                zo_config_home.join(leaf),
            );
        }
    }

    if let Some(zo_home) = env::var_os("ZO_HOME") {
        let zo_home = PathBuf::from(zo_home);
        if !zo_home.as_os_str().is_empty() {
            push_unique_root(
                &mut roots,
                DefinitionSource::UserZoHome,
                zo_home.join(leaf),
            );
        }
    }

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        if !home.as_os_str().is_empty() {
            push_unique_root(
                &mut roots,
                DefinitionSource::UserZo,
                home.join(".zo").join(leaf),
            );
        }
    }

    roots
}

pub(crate) fn discover_skill_roots(cwd: &Path) -> Vec<SkillRoot> {
    discover_skill_roots_from(
        cwd,
        env::var_os("ZO_CONFIG_HOME").map(PathBuf::from),
        env::var_os("ZO_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

pub(crate) fn discover_skill_roots_from(
    cwd: &Path,
    zo_config_home: Option<PathBuf>,
    zo_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<SkillRoot> {
    let mut roots = Vec::new();

    for ancestor in cwd.ancestors() {
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectZo,
            ancestor.join(".zo").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectZo,
            ancestor.join(".zo").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    // Each global tier keeps its own `DefinitionSource` tag for provenance
    // display, so this discovery enumerates the tiers itself. Empty values are
    // skipped exactly like `core_types::paths::zo_global_config_roots`.
    if let Some(zo_config_home) = zo_config_home.filter(|path| !path.as_os_str().is_empty()) {
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZoConfigHome,
            zo_config_home.join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZoConfigHome,
            zo_config_home.join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    if let Some(zo_home) = zo_home.filter(|path| !path.as_os_str().is_empty()) {
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZoHome,
            zo_home.join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZoHome,
            zo_home.join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    if let Some(home) = home.filter(|path| !path.as_os_str().is_empty()) {
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZo,
            home.join(".zo").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserZo,
            home.join(".zo").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    roots
}

fn install_skill(source: &str, cwd: &Path) -> std::io::Result<InstalledSkill> {
    let registry_root = default_skill_install_root()?;
    install_skill_into(source, cwd, &registry_root)
}

pub(crate) fn install_skill_into(
    source: &str,
    cwd: &Path,
    registry_root: &Path,
) -> std::io::Result<InstalledSkill> {
    let source = resolve_skill_install_source(source, cwd)?;
    let prompt_path = source.prompt_path();
    let contents = fs::read_to_string(prompt_path)?;
    let display_name = parse_skill_frontmatter(&contents).0;
    let invocation_name = derive_skill_install_name(&source, display_name.as_deref())?;
    let installed_path = registry_root.join(&invocation_name);

    if installed_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "skill '{invocation_name}' is already installed at {}",
                installed_path.display()
            ),
        ));
    }

    fs::create_dir_all(&installed_path)?;
    let install_result = match &source {
        SkillInstallSource::Directory { root, .. } => {
            copy_directory_contents(root, &installed_path)
        }
        SkillInstallSource::MarkdownFile { path } => {
            fs::copy(path, installed_path.join("SKILL.md")).map(|_| ())
        }
    };
    if let Err(error) = install_result {
        let _ = fs::remove_dir_all(&installed_path);
        return Err(error);
    }

    Ok(InstalledSkill {
        invocation_name,
        display_name,
        source: source.report_path().to_path_buf(),
        registry_root: registry_root.to_path_buf(),
        installed_path,
    })
}

fn default_skill_install_root() -> std::io::Result<PathBuf> {
    if let Some(home) = core_types::paths::zo_global_config_roots()
        .into_iter()
        .next()
    {
        return Ok(home.join("skills"));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "unable to resolve a skills install root; set ZO_CONFIG_HOME, ZO_HOME, or HOME",
    ))
}

fn resolve_skill_install_source(source: &str, cwd: &Path) -> std::io::Result<SkillInstallSource> {
    let candidate = PathBuf::from(source);
    let source = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    let source = fs::canonicalize(&source)?;

    if source.is_dir() {
        let prompt_path = source.join("SKILL.md");
        if !prompt_path.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "skill directory '{}' must contain SKILL.md",
                    source.display()
                ),
            ));
        }
        return Ok(SkillInstallSource::Directory {
            root: source,
            prompt_path,
        });
    }

    if source
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("md"))
    {
        return Ok(SkillInstallSource::MarkdownFile { path: source });
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "skill source '{}' must be a directory with SKILL.md or a markdown file",
            source.display()
        ),
    ))
}

fn derive_skill_install_name(
    source: &SkillInstallSource,
    declared_name: Option<&str>,
) -> std::io::Result<String> {
    for candidate in [declared_name, source.fallback_name().as_deref()] {
        if let Some(candidate) = candidate.and_then(sanitize_skill_invocation_name) {
            return Ok(candidate);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "unable to derive an installable invocation name from '{}'",
            source.report_path().display()
        ),
    ))
}

fn sanitize_skill_invocation_name(candidate: &str) -> Option<String> {
    let trimmed = candidate
        .trim()
        .trim_start_matches('/')
        .trim_start_matches('$');
    if trimmed.is_empty() {
        return None;
    }

    let mut sanitized = String::new();
    let mut last_was_separator = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if (ch.is_whitespace() || matches!(ch, '/' | '\\'))
            && !last_was_separator
            && !sanitized.is_empty()
        {
            sanitized.push('-');
            last_was_separator = true;
        }
    }

    let sanitized = sanitized
        .trim_matches(|ch| matches!(ch, '-' | '_' | '.'))
        .to_string();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn copy_directory_contents(source: &Path, destination: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let destination_path = destination.join(entry.file_name());
        if entry_type.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_directory_contents(&entry.path(), &destination_path)?;
        } else {
            fs::copy(entry.path(), destination_path)?;
        }
    }
    Ok(())
}

impl SkillInstallSource {
    fn prompt_path(&self) -> &Path {
        match self {
            Self::Directory { prompt_path, .. } => prompt_path,
            Self::MarkdownFile { path } => path,
        }
    }

    fn fallback_name(&self) -> Option<String> {
        match self {
            Self::Directory { root, .. } => root
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            Self::MarkdownFile { path } => path
                .file_stem()
                .map(|name| name.to_string_lossy().to_string()),
        }
    }

    fn report_path(&self) -> &Path {
        match self {
            Self::Directory { root, .. } => root,
            Self::MarkdownFile { path } => path,
        }
    }
}

fn push_unique_root(
    roots: &mut Vec<(DefinitionSource, PathBuf)>,
    source: DefinitionSource,
    path: PathBuf,
) {
    if path.is_dir() && !roots.iter().any(|(_, existing)| existing == &path) {
        roots.push((source, path));
    }
}

fn push_unique_skill_root(
    roots: &mut Vec<SkillRoot>,
    source: DefinitionSource,
    path: PathBuf,
    origin: SkillOrigin,
) {
    if path.is_dir() && !roots.iter().any(|existing| existing.path == path) {
        roots.push(SkillRoot {
            source,
            path,
            origin,
        });
    }
}

// 단일 root 의 read_dir 실패(권한 없음·삭제됨)나 개별 파일 읽기 실패가 다른
// 정상 소스의 에이전트까지 잃게 만들지 않도록, io 에러를 디렉터리·파일 단위로
// 흡수해 가능한 항목만 수집한다. 그 결과 현재 Err 를 반환할 일은 없지만,
// 대칭 함수 `load_skills_from_roots` 및 호출처와의 인터페이스 안정성을 위해
// `io::Result` 시그니처를 유지한다.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn load_agents_from_roots(
    roots: &[(DefinitionSource, PathBuf)],
) -> std::io::Result<Vec<AgentSummary>> {
    let mut agents = Vec::new();
    let mut active_sources = BTreeMap::<String, DefinitionSource>::new();

    for (source, root) in roots {
        let mut root_agents = Vec::new();
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            let extension = extension.to_ascii_lowercase();
            if extension != "toml" && extension != "md" {
                continue;
            }
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            let fallback_name = path.file_stem().map_or_else(
                || entry.file_name().to_string_lossy().to_string(),
                |stem| stem.to_string_lossy().to_string(),
            );
            let (name, description, model, reasoning_effort) = if extension == "md" {
                if NATIVE_AGENT_BUILTIN_TYPES.contains(&fallback_name.as_str()) {
                    continue;
                }
                let Some(agent) = parse_native_markdown_agent_summary(&contents) else {
                    continue;
                };
                (fallback_name, agent.description, agent.model, None)
            } else {
                (
                    parse_toml_string(&contents, "name").unwrap_or(fallback_name),
                    parse_toml_string(&contents, "description"),
                    parse_toml_string(&contents, "model"),
                    parse_toml_string(&contents, "model_reasoning_effort"),
                )
            };
            root_agents.push(AgentSummary {
                name,
                description,
                model,
                reasoning_effort,
                source: *source,
                shadowed_by: None,
            });
        }
        root_agents.sort_by(|left, right| left.name.cmp(&right.name));

        for mut agent in root_agents {
            let key = agent.name.to_ascii_lowercase();
            if let Some(existing) = active_sources.get(&key) {
                agent.shadowed_by = Some(*existing);
            } else {
                active_sources.insert(key, agent.source);
            }
            agents.push(agent);
        }
    }

    Ok(agents)
}

// `load_agents_from_roots` 와 동일하게, 한 root 의 read_dir 실패나 개별
// SKILL.md 읽기 실패를 디렉터리·파일 단위로 흡수해 가능한 스킬만 수집한다.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn load_skills_from_roots(roots: &[SkillRoot]) -> std::io::Result<Vec<SkillSummary>> {
    let mut skills = Vec::new();
    let mut active_sources = BTreeMap::<String, DefinitionSource>::new();

    for root in roots {
        let mut root_skills = Vec::new();
        let Ok(entries) = fs::read_dir(&root.path) else {
            continue;
        };
        for entry in entries.flatten() {
            match root.origin {
                SkillOrigin::SkillsDir => {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    let skill_path = entry.path().join("SKILL.md");
                    if !skill_path.is_file() {
                        continue;
                    }
                    let Ok(contents) = fs::read_to_string(skill_path) else {
                        continue;
                    };
                    let (name, description) = parse_skill_frontmatter(&contents);
                    root_skills.push(SkillSummary {
                        name: name
                            .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string()),
                        description,
                        source: root.source,
                        shadowed_by: None,
                        origin: root.origin,
                    });
                }
                SkillOrigin::LegacyCommandsDir => {
                    let path = entry.path();
                    let markdown_path = if path.is_dir() {
                        let skill_path = path.join("SKILL.md");
                        if !skill_path.is_file() {
                            continue;
                        }
                        skill_path
                    } else if path
                        .extension()
                        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("md"))
                    {
                        path
                    } else {
                        continue;
                    };

                    let Ok(contents) = fs::read_to_string(&markdown_path) else {
                        continue;
                    };
                    let fallback_name = markdown_path.file_stem().map_or_else(
                        || entry.file_name().to_string_lossy().to_string(),
                        |stem| stem.to_string_lossy().to_string(),
                    );
                    let (name, description) = parse_skill_frontmatter(&contents);
                    root_skills.push(SkillSummary {
                        name: name.unwrap_or(fallback_name),
                        description,
                        source: root.source,
                        shadowed_by: None,
                        origin: root.origin,
                    });
                }
            }
        }
        root_skills.sort_by(|left, right| left.name.cmp(&right.name));

        for mut skill in root_skills {
            let key = skill.name.to_ascii_lowercase();
            if let Some(existing) = active_sources.get(&key) {
                skill.shadowed_by = Some(*existing);
            } else {
                active_sources.insert(key, skill.source);
            }
            skills.push(skill);
        }
    }

    Ok(skills)
}

pub(crate) fn parse_toml_string(contents: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} =");
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(value) = trimmed.strip_prefix(&prefix) else {
            continue;
        };
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeMarkdownAgentSummary {
    description: Option<String>,
    model: Option<String>,
}

pub(crate) fn parse_native_markdown_agent_summary(
    contents: &str,
) -> Option<NativeMarkdownAgentSummary> {
    let contents = contents.trim_start_matches('\u{feff}');
    let mut description = None;
    let mut model = None;

    let body = if let Some(after_open) = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
    {
        match split_markdown_frontmatter(after_open) {
            Some((frontmatter, body)) => {
                for line in frontmatter.lines() {
                    let Some((key, value)) = line.split_once(':') else {
                        continue;
                    };
                    let key = key.trim().to_ascii_lowercase();
                    let value = unquote_frontmatter_value(value.trim());
                    match key.as_str() {
                        "description" if !value.is_empty() => description = Some(value),
                        "model" if !value.is_empty() => model = Some(value),
                        "permission" => validate_native_agent_permission_rules(&value)?,
                        "permissionmode" | "permission_mode" => {
                            PermissionMode::parse(&value)?;
                        }
                        // Native custom-agent files may include `name` and
                        // `tools`; `/agents` lists the invocation name from
                        // the file stem and does not render tool allowlists.
                        _ => {}
                    }
                }
                body.trim()
            }
            None => contents.trim(),
        }
    } else {
        contents.trim()
    };

    if body.is_empty() && description.is_none() {
        return None;
    }

    Some(NativeMarkdownAgentSummary { description, model })
}

fn validate_native_agent_permission_rules(value: &str) -> Option<()> {
    let mut saw_rule = false;
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (rule, decision) = token.rsplit_once('=')?;
        if rule.trim().is_empty() {
            return None;
        }
        match decision.trim().to_ascii_lowercase().as_str() {
            "allow" | "deny" | "ask" => saw_rule = true,
            _ => return None,
        }
    }
    saw_rule.then_some(())
}

fn split_markdown_frontmatter(after_open: &str) -> Option<(&str, &str)> {
    let mut index = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            return Some((&after_open[..index], &after_open[index + line.len()..]));
        }
        index += line.len();
    }
    None
}

pub(crate) fn parse_skill_frontmatter(contents: &str) -> (Option<String>, Option<String>) {
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }

    let mut name = None;
    let mut description = None;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            let value = unquote_frontmatter_value(value.trim());
            if !value.is_empty() {
                name = Some(value);
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            let value = unquote_frontmatter_value(value.trim());
            if !value.is_empty() {
                description = Some(value);
            }
        }
    }

    (name, description)
}

fn unquote_frontmatter_value(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|trimmed| trimmed.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|trimmed| trimmed.strip_suffix('\''))
        })
        .unwrap_or(value)
        .trim()
        .to_string()
}

pub(crate) fn render_agents_report(agents: &[AgentSummary]) -> String {
    if agents.is_empty() {
        return "No agents found.".to_string();
    }

    let total_active = agents
        .iter()
        .filter(|agent| agent.shadowed_by.is_none())
        .count();
    let mut lines = vec![
        "Agents".to_string(),
        format!("  {total_active} active agents"),
        String::new(),
    ];

    for source in [
        DefinitionSource::ProjectZo,
        DefinitionSource::UserZoConfigHome,
        DefinitionSource::UserZoHome,
        DefinitionSource::UserZo,
    ] {
        let group = agents
            .iter()
            .filter(|agent| agent.source == source)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }

        lines.push(format!("{}:", source.label()));
        for agent in group {
            let detail = agent_detail(agent);
            match agent.shadowed_by {
                Some(winner) => lines.push(format!("  (shadowed by {}) {detail}", winner.label())),
                None => lines.push(format!("  {detail}")),
            }
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

fn agent_detail(agent: &AgentSummary) -> String {
    let mut parts = vec![agent.name.clone()];
    if let Some(description) = &agent.description {
        parts.push(description.clone());
    }
    if let Some(model) = &agent.model {
        parts.push(model.clone());
    }
    if let Some(reasoning) = &agent.reasoning_effort {
        parts.push(reasoning.clone());
    }
    parts.join(" · ")
}

pub(crate) fn render_skills_report(skills: &[SkillSummary]) -> String {
    if skills.is_empty() {
        return "No skills found.".to_string();
    }

    let total_active = skills
        .iter()
        .filter(|skill| skill.shadowed_by.is_none())
        .count();
    let mut lines = vec![
        "Skills".to_string(),
        format!("  {total_active} available skills"),
        String::new(),
    ];

    for source in [
        DefinitionSource::ProjectZo,
        DefinitionSource::UserZoConfigHome,
        DefinitionSource::UserZoHome,
        DefinitionSource::UserZo,
    ] {
        let group = skills
            .iter()
            .filter(|skill| skill.source == source)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }

        lines.push(format!("{}:", source.label()));
        for skill in group {
            let mut parts = vec![skill.name.clone()];
            if let Some(description) = &skill.description {
                parts.push(description.clone());
            }
            if let Some(detail) = skill.origin.detail_label() {
                parts.push(detail.to_string());
            }
            let detail = parts.join(" · ");
            match skill.shadowed_by {
                Some(winner) => lines.push(format!("  (shadowed by {}) {detail}", winner.label())),
                None => lines.push(format!("  {detail}")),
            }
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

pub(crate) fn render_skill_install_report(skill: &InstalledSkill) -> String {
    let mut lines = vec![
        "Skills".to_string(),
        format!("  Result           installed {}", skill.invocation_name),
        format!("  Invoke as        ${}", skill.invocation_name),
    ];
    if let Some(display_name) = &skill.display_name {
        lines.push(format!("  Display name     {display_name}"));
    }
    lines.push(format!("  Source           {}", skill.source.display()));
    lines.push(format!(
        "  Registry         {}",
        skill.registry_root.display()
    ));
    lines.push(format!(
        "  Installed path   {}",
        skill.installed_path.display()
    ));
    lines.join("\n")
}

fn render_mcp_summary_report(
    cwd: &Path,
    servers: &BTreeMap<String, ScopedMcpServerConfig>,
) -> String {
    let field_width = report_field_width(&["Working directory", "Configured servers"]);
    let mut lines = vec![
        "MCP".to_string(),
        report_row(field_width, "Working directory", cwd.display()),
        report_row(field_width, "Configured servers", servers.len()),
    ];
    if servers.is_empty() {
        lines.push("  No MCP servers configured.".to_string());
        return lines.join("\n");
    }

    lines.push(String::new());
    for (name, server) in servers {
        lines.push(format!(
            "  {name:<16} {transport:<13} {scope:<7} {summary}",
            transport = mcp_transport_label(&server.config),
            scope = config_source_label(server.scope),
            summary = mcp_server_summary(&server.config)
        ));
    }

    lines.join("\n")
}

/// Render the "blocked (untrusted)" section for project-scoped MCP servers the
/// supply-chain trust gate skipped, with the actions that enable them. Returns
/// `None` when nothing was gated, so the summary stays clean for the common case.
///
/// This makes a deliberately-gated server (a repo-committed `.zo/settings.json`
/// / `.zo/mcp.json` server that is not yet trusted) visible and actionable, instead
/// of silently vanishing from the configured list — the reported "MCP 연결 안됨"
/// where a configured server never appears.
fn render_blocked_mcp_section(untrusted: &[&UntrustedMcpServer]) -> Option<String> {
    if untrusted.is_empty() {
        return None;
    }
    let mut lines = vec![format!(
        "  Blocked (untrusted) {}",
        untrusted.len()
    )];
    for server in untrusted {
        lines.push(format!(
            "  {name:<16} {path}",
            name = server.name,
            path = server.path.display()
        ));
    }
    lines.push(String::new());
    lines.push(
        "  These project-scoped servers are gated for supply-chain safety. To enable one:"
            .to_string(),
    );
    lines.push(
        "    • add its name to .zo/trusted-mcp-servers.json, or".to_string(),
    );
    lines.push(
        "    • set \"enableAllProjectMcpServers\": true in a trusted (User/local) settings file, or"
            .to_string(),
    );
    lines.push("    • move it to .zo/settings.local.json (git-ignored, operator-authored).".to_string());
    Some(lines.join("\n"))
}

fn render_mcp_server_report(
    cwd: &Path,
    server_name: &str,
    server: Option<&ScopedMcpServerConfig>,
) -> String {
    let field_width = report_field_width(MCP_SERVER_REPORT_LABELS);
    let Some(server) = server else {
        return [
            "MCP".to_string(),
            report_row(field_width, "Working directory", cwd.display()),
            report_row(
                field_width,
                "Result",
                format!("server `{server_name}` is not configured"),
            ),
        ]
        .join("\n");
    };

    let mut lines = vec![
        "MCP".to_string(),
        report_row(field_width, "Working directory", cwd.display()),
        report_row(field_width, "Name", server_name),
        report_row(field_width, "Scope", config_source_label(server.scope)),
        report_row(
            field_width,
            "Transport",
            mcp_transport_label(&server.config),
        ),
    ];

    match &server.config {
        McpServerConfig::Stdio(config) => {
            lines.push(report_row(field_width, "Command", &config.command));
            lines.push(report_row(
                field_width,
                "Args",
                format_optional_list(&config.args),
            ));
            lines.push(report_row(
                field_width,
                "Env keys",
                format_optional_keys(config.env.keys().cloned().collect()),
            ));
            lines.push(report_row(
                field_width,
                "Tool timeout",
                config
                    .tool_call_timeout_ms
                    .map_or_else(|| "<default>".to_string(), |value| format!("{value} ms")),
            ));
        }
        McpServerConfig::Sse(config) | McpServerConfig::Http(config) => {
            lines.push(report_row(field_width, "URL", &config.url));
            lines.push(report_row(
                field_width,
                "Header keys",
                format_optional_keys(config.headers.keys().cloned().collect()),
            ));
            lines.push(report_row(
                field_width,
                "Header helper",
                config.headers_helper.as_deref().unwrap_or("<none>"),
            ));
            lines.push(report_row(
                field_width,
                "OAuth",
                format_mcp_oauth(config.oauth.as_ref()),
            ));
        }
        McpServerConfig::Ws(config) => {
            lines.push(report_row(field_width, "URL", &config.url));
            lines.push(report_row(
                field_width,
                "Header keys",
                format_optional_keys(config.headers.keys().cloned().collect()),
            ));
            lines.push(report_row(
                field_width,
                "Header helper",
                config.headers_helper.as_deref().unwrap_or("<none>"),
            ));
        }
        McpServerConfig::Sdk(config) => {
            lines.push(report_row(field_width, "SDK name", &config.name));
        }
        McpServerConfig::ManagedProxy(config) => {
            lines.push(report_row(field_width, "URL", &config.url));
            lines.push(report_row(field_width, "Proxy id", &config.id));
        }
    }

    lines.join("\n")
}

fn render_agents_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        "Agents".to_string(),
        "  Usage            /agents [list|help]".to_string(),
        "  Direct CLI       zo agents".to_string(),
        "  Sources          .zo/agents, $ZO_CONFIG_HOME/agents, $ZO_HOME/agents, ~/.zo/agents".to_string(),
    ];
    if let Some(args) = unexpected {
        lines.push(format!("  Unexpected       {args}"));
    }
    lines.join("\n")
}

fn render_skills_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        "Skills".to_string(),
        "  Usage            /skills [list|install <path>|help]".to_string(),
        "  Direct CLI       zo skills [list|install <path>|help]".to_string(),
        "  Install root     $ZO_CONFIG_HOME/skills, $ZO_HOME/skills, or ~/.zo/skills"
            .to_string(),
        "  Sources          .zo/skills and .zo/commands".to_string(),
    ];
    if let Some(args) = unexpected {
        lines.push(format!("  Unexpected       {args}"));
    }
    lines.join("\n")
}

pub(crate) fn render_mcp_auth_list(
    cwd: &Path,
    servers: &BTreeMap<String, ScopedMcpServerConfig>,
) -> String {
    let field_width = report_field_width(&["Working directory"]);
    let mut lines = vec![
        "MCP auth".to_string(),
        report_row(field_width, "Working directory", cwd.display()),
    ];
    let oauth_servers: Vec<&String> = servers
        .iter()
        .filter(|(_, server)| mcp_server_oauth_capable(&server.config))
        .map(|(name, _)| name)
        .collect();
    if oauth_servers.is_empty() {
        lines.push("  No OAuth-capable MCP servers configured.".to_string());
        return lines.join("\n");
    }
    lines.push(String::new());
    for name in oauth_servers {
        lines.push(format!("  {name:<16} {}", mcp_token_status_label(name)));
    }
    lines.join("\n")
}

/// Whether `zo mcp auth` can authenticate this server. Every remote HTTP/SSE
/// server qualifies: it either declares an explicit `oauth` block or supports
/// bounded native discovery. Determining this is purely local — no network call
/// is made just to list servers.
fn mcp_server_oauth_capable(config: &McpServerConfig) -> bool {
    matches!(config, McpServerConfig::Sse(_) | McpServerConfig::Http(_))
}

fn mcp_token_status_label(server_name: &str) -> &'static str {
    match runtime::load_mcp_oauth_token(server_name) {
        Ok(Some(token)) if runtime::is_mcp_token_expired(&token) => "authenticated (expired)",
        Ok(Some(_)) => "authenticated",
        Ok(None) => "not authenticated",
        Err(_) => "status unavailable",
    }
}

fn render_mcp_auth(cwd: &Path, server_name: &str, config: &RuntimeConfig) -> String {
    let field_width =
        report_field_width(&["Working directory", "Name", "Result", "Auth", "Detail"]);
    let Some(scoped) = config.mcp().get(server_name) else {
        return [
            "MCP".to_string(),
            report_row(field_width, "Working directory", cwd.display()),
            report_row(field_width, "Name", server_name),
            report_row(
                field_width,
                "Result",
                format!("server `{server_name}` is not configured"),
            ),
        ]
        .join("\n");
    };
    let result = match &scoped.config {
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => {
            authenticate_mcp_server_remote(server_name, remote, &LocalBrowserOpener)
        }
        _ => {
            return [
                "MCP".to_string(),
                report_row(field_width, "Working directory", cwd.display()),
                report_row(field_width, "Name", server_name),
                report_row(
                    field_width,
                    "Result",
                    "server transport does not support MCP OAuth",
                ),
            ]
            .join("\n");
        }
    };

    let (status, detail) = match result {
        McpAuthResult::AlreadyAuthenticated { .. } => (
            "already authenticated",
            "a valid token is already present".to_string(),
        ),
        McpAuthResult::Authenticated { scopes, .. } => (
            "authenticated",
            if scopes.is_empty() {
                "OAuth flow complete".to_string()
            } else {
                format!("scopes: {}", scopes.join(", "))
            },
        ),
        McpAuthResult::Refreshed { .. } => ("refreshed", "cached token refreshed".to_string()),
        McpAuthResult::Failed { reason, .. } => ("failed", reason),
    };
    [
        "MCP".to_string(),
        report_row(field_width, "Working directory", cwd.display()),
        report_row(field_width, "Name", server_name),
        report_row(field_width, "Auth", status),
        report_row(field_width, "Detail", detail),
    ]
    .join("\n")
}

fn render_mcp_logout(cwd: &Path, server_name: &str) -> String {
    let had_token = matches!(runtime::load_mcp_oauth_token(server_name), Ok(Some(_)));
    let result = match runtime::clear_mcp_oauth_token(server_name) {
        Ok(()) if had_token => "removed stored MCP OAuth credentials".to_string(),
        Ok(()) => "no stored credentials to remove".to_string(),
        Err(error) => format!("failed to remove credentials: {error}"),
    };
    let field_width = report_field_width(&["Working directory", "Name", "Result"]);
    [
        "MCP".to_string(),
        report_row(field_width, "Working directory", cwd.display()),
        report_row(field_width, "Name", server_name),
        report_row(field_width, "Result", result),
    ]
    .join("\n")
}

fn render_mcp_usage(unexpected: Option<&str>) -> String {
    use crate::mcp_command::MCP_FULL_USAGE;
    let mut lines = vec![
        "MCP".to_string(),
        format!("  Usage            {MCP_FULL_USAGE}"),
        format!(
            "  Direct CLI       zo mcp {}",
            MCP_FULL_USAGE.trim_start_matches("/mcp ")
        ),
        "  Auth             /mcp auth list shows OAuth-capable servers; /mcp auth <server> runs the flow"
            .to_string(),
        "  Sources          ~/.zo/settings.json (global), .zo/settings.json (project), .zo/settings.local.json (local)".to_string(),
    ];
    if let Some(args) = unexpected {
        lines.push(format!("  Unexpected       {args}"));
    }
    lines.join("\n")
}

fn config_source_label(source: ConfigSource) -> &'static str {
    match source {
        ConfigSource::User => "user",
        ConfigSource::Project => "project",
        ConfigSource::Local => "local",
    }
}

fn mcp_transport_label(config: &McpServerConfig) -> &'static str {
    match config {
        McpServerConfig::Stdio(_) => "stdio",
        McpServerConfig::Sse(_) => "sse",
        McpServerConfig::Http(_) => "http",
        McpServerConfig::Ws(_) => "ws",
        McpServerConfig::Sdk(_) => "sdk",
        McpServerConfig::ManagedProxy(_) => "managed-proxy",
    }
}

fn mcp_server_summary(config: &McpServerConfig) -> String {
    match config {
        McpServerConfig::Stdio(config) => {
            if config.args.is_empty() {
                config.command.clone()
            } else {
                format!("{} {}", config.command, config.args.join(" "))
            }
        }
        McpServerConfig::Sse(config) | McpServerConfig::Http(config) => config.url.clone(),
        McpServerConfig::Ws(config) => config.url.clone(),
        McpServerConfig::Sdk(config) => config.name.clone(),
        McpServerConfig::ManagedProxy(config) => format!("{} ({})", config.id, config.url),
    }
}

fn format_optional_list(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(" ")
    }
}

fn format_optional_keys(mut keys: Vec<String>) -> String {
    if keys.is_empty() {
        return "<none>".to_string();
    }
    keys.sort();
    keys.join(", ")
}

fn format_mcp_oauth(oauth: Option<&McpOAuthConfig>) -> String {
    let Some(oauth) = oauth else {
        return "<none>".to_string();
    };

    let mut parts = Vec::new();
    if let Some(client_id) = &oauth.client_id {
        parts.push(format!("client_id={client_id}"));
    }
    if let Some(port) = oauth.callback_port {
        parts.push(format!("callback_port={port}"));
    }
    if let Some(url) = &oauth.auth_server_metadata_url {
        parts.push(format!("metadata_url={url}"));
    }
    if let Some(xaa) = oauth.xaa {
        parts.push(format!("xaa={xaa}"));
    }
    if parts.is_empty() {
        "enabled".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_value_column(report: &str, label: &str, value: &str, expected: usize) {
        let line = report
            .lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("missing {label:?} row in {report:?}"));
        assert_eq!(line.find(value), Some(expected), "{line:?}");
    }

    #[test]
    fn plugin_reports_keep_the_seventeen_cell_label_field() {
        let report = render_plugin_install_report("demo-plugin", None);
        assert_value_column(&report, "Result", "installed", 2 + MIN_REPORT_LABEL_FIELD);
        assert_value_column(&report, "Name", "demo-plugin", 2 + MIN_REPORT_LABEL_FIELD);
        assert_value_column(&report, "Version", "unknown", 2 + MIN_REPORT_LABEL_FIELD);
        assert_value_column(&report, "Status", "disabled", 2 + MIN_REPORT_LABEL_FIELD);
    }

    #[test]
    fn mcp_server_rows_align_under_the_longest_server_label() {
        let report = render_mcp_server_report(Path::new("report-root"), "demo", None);
        let expected = 2 + report_field_width(&["Working directory", "Result"]);
        assert_eq!(expected, 20);
        assert_value_column(&report, "Working directory", "report-root", expected);
        assert_value_column(&report, "Result", "server `demo`", expected);
    }

    #[test]
    fn mcp_summary_expands_for_configured_servers() {
        let report = render_mcp_summary_report(Path::new("report-root"), &BTreeMap::new());
        let expected = 2 + report_field_width(&["Working directory", "Configured servers"]);
        assert_eq!(expected, 21);
        assert_value_column(&report, "Working directory", "report-root", expected);
        assert_value_column(&report, "Configured servers", "0", expected);
    }
}
