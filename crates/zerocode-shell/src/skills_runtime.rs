//! Skill discovery, evidence and plans. Commands and Settings only delegate.
use super::*;
use std::io::Read;
use zerocode_core::skill::{
    self, Policy, ScannedSource, Skill, SkillFamily, SkillReport, SkillSource, SourceKind, Usage,
};

const USAGE_FILE: &str = "skills-usage.json";
const PLANS_FILE: &str = "skills-plans.json";

#[derive(Clone)]
pub(crate) struct Context {
    home: PathBuf,
    repos: Vec<PathBuf>,
    data: PathBuf,
    policy: Policy,
}
impl Context {
    pub(crate) fn from_state(state: &AppState) -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("skills.homeUnavailable")?;
        let policy = Policy::overlay(&load_settings(state.settings())?.document.skills);
        let mut repos = vec![state.active_root()];
        for repo in stored_projects(state.config_root())
            .into_iter()
            .take(policy.max_roots)
        {
            let path = PathBuf::from(repo);
            if !repos.contains(&path) {
                repos.push(path);
            }
        }
        Ok(Self {
            home,
            repos,
            data: state.local_data_root().to_path_buf(),
            policy,
        })
    }
    fn sources(&self) -> Vec<SkillSource> {
        let mut sources = skill::discovery_sources(&self.home, &self.repos);
        // The active Codex home can differ from ~/.codex (account/runtime home).
        if let Some(home) = std::env::var_os("CODEX_HOME")
            && let Some(mut source) = sources.iter().find(|s| s.id == "home-codex").cloned()
        {
            source.path = PathBuf::from(home).join("skills");
            if !sources.iter().any(|s| s.path == source.path) {
                sources.insert(0, source);
            }
        }
        sources.truncate(self.policy.max_roots);
        sources
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub id: String,
    pub name: String,
    pub path: Option<PathBuf>,
    pub agent: String,
    pub pane: String,
    pub at_ms: i64,
    pub source: String,
}
impl Evidence {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.id.len()
            + self.name.len()
            + self.agent.len()
            + self.pane.len()
            + self.source.len()
            + self.path.as_ref().map_or(0, |p| p.as_os_str().len())
    }
}
/// Both adapters produce only explicit, timestamped load/read evidence. An
/// Available Skills index without a load timestamp proves availability only.
trait EvidenceAdapter {
    fn available(&self) -> bool {
        true
    }
    fn read(&self, policy: &Policy) -> Vec<Evidence>;
}
struct HookRingAdapter<'a>(&'a Path);
impl EvidenceAdapter for HookRingAdapter<'_> {
    fn read(&self, policy: &Policy) -> Vec<Evidence> {
        read_bounded(self.0, policy.max_evidence_bytes)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Evidence>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .take(policy.max_usage_events)
            .collect()
    }
}
/// zo::skill_sources never persists the prompt index; --prompt-input is a
/// budget/availability report. Keep this adapter explicit and unavailable until
/// zo ships timestamped load evidence. Never infer use from availability.
struct ZoIndexAdapter;
impl EvidenceAdapter for ZoIndexAdapter {
    fn available(&self) -> bool {
        false
    }
    fn read(&self, _policy: &Policy) -> Vec<Evidence> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
struct Runtime {
    roots: HashMap<PathBuf, skill::ScanSnapshot>,
    usage: VecDeque<Evidence>,
    loaded_data: Option<PathBuf>,
    plans: VecDeque<Plan>,
}
static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();
fn runtime() -> MutexGuard<'static, Runtime> {
    RUNTIME
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
impl Runtime {
    fn ingest(&mut self, evidence: impl IntoIterator<Item = Evidence>, policy: &Policy, now: i64) {
        let cutoff = now.saturating_sub(policy.usage_window_ms);
        self.usage.retain(|e| e.at_ms >= cutoff && e.at_ms <= now);
        for mut event in evidence {
            if event.at_ms < cutoff
                || event.at_ms > now
                || self.usage.iter().any(|e| e.id == event.id)
            {
                continue;
            }
            // External evidence cannot defeat the ring's byte bound with a name.
            event.name = event.name.chars().take(policy.summary_chars).collect();
            event.agent = event.agent.chars().take(policy.summary_chars).collect();
            event.pane = event.pane.chars().take(policy.summary_chars).collect();
            self.usage.push_back(event);
            while self.usage.len() > policy.max_usage_events
                || self
                    .usage
                    .iter()
                    .map(Evidence::retained_bytes)
                    .sum::<usize>()
                    > policy.max_evidence_bytes
            {
                self.usage.pop_front();
            }
        }
    }
    fn load_evidence(&mut self, context: &Context) {
        let now = now_epoch_ms();
        if self.loaded_data.as_ref() != Some(&context.data) {
            self.usage.clear();
            self.plans.clear();
            self.ingest(
                HookRingAdapter(&context.data.join(USAGE_FILE)).read(&context.policy),
                &context.policy,
                now,
            );
            self.plans = read_bounded(
                &context.data.join(PLANS_FILE),
                context.policy.max_plan_bytes,
            )
            .ok()
            .and_then(|s| serde_json::from_str::<VecDeque<Plan>>(&s).ok())
            .unwrap_or_default();
            self.plans.truncate(context.policy.max_plans);
            self.loaded_data = Some(context.data.clone());
        }
        self.ingest(ZoIndexAdapter.read(&context.policy), &context.policy, now);
        self.ingest([], &context.policy, now);
    }
    fn scan(
        &mut self,
        sources: Vec<SkillSource>,
        policy: &Policy,
        force: bool,
    ) -> (SkillReport, usize, bool) {
        let keep: HashSet<_> = sources.iter().map(|s| s.path.clone()).collect();
        self.roots.retain(|path, _| keep.contains(path));
        let mut report = SkillReport::default();
        let mut reads = 0;
        let mut capped = false;
        for source in sources {
            let old = self.roots.remove(&source.path).filter(|_| !force);
            let scanned = skill::scan_incremental(&source, policy, old);
            reads += scanned.reads;
            capped |= scanned.capped;
            report.sources.push(ScannedSource {
                exists: source.path.is_dir(),
                found: scanned.skills.len(),
                source: source.clone(),
            });
            report.skills.extend(scanned.skills.iter().cloned());
            self.roots.insert(source.path, scanned);
        }
        (report, reads, capped)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct RequiredSkill {
    pub name: String,
    pub missing_agents: Vec<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub families: Vec<SkillFamily>,
    pub sources: Vec<ScannedSource>,
    pub agents: Vec<(String, String)>,
    pub external_agents: Vec<(String, String)>,
    pub external_unavailable: Vec<String>,
    pub installed_agents_by_skill: BTreeMap<String, Vec<String>>,
    pub required: Vec<RequiredSkill>,
    pub policy: Policy,
    pub reads: usize,
    pub capped: bool,
    pub evidence_count: usize,
    pub evidence_scope: &'static str,
    pub plans: Vec<Plan>,
    pub retained_evidence_bytes: usize,
}
pub(crate) fn list(context: Context, force: bool) -> Report {
    let mut held = runtime();
    held.load_evidence(&context);
    let (found, reads, capped) = held.scan(context.sources(), &context.policy, force);
    let agents = installed_agents();
    let external_agents = agents
        .iter()
        .filter(|(id, _)| skill::cli_agent(id).is_some())
        .cloned()
        .collect();
    let external_unavailable = agents
        .iter()
        .filter(|(id, _)| skill::cli_agent(id).is_none())
        .map(|(_, label)| label.clone())
        .collect();
    let required = context
        .policy
        .required_names()
        .into_iter()
        .map(|name| {
            let missing_agents = agents
                .iter()
                .filter(|(agent, _)| {
                    !found.skills.iter().any(|s| {
                        s.name.eq_ignore_ascii_case(&name)
                            && s.source_kind != SourceKind::Repo
                            && skill::agent_reads_skill(agent, s)
                    })
                })
                .map(|(agent, _)| agent.clone())
                .collect();
            RequiredSkill {
                name,
                missing_agents,
            }
        })
        .collect();
    let installed_agents_by_skill = found
        .skills
        .iter()
        .map(|variant| {
            let readers = agents
                .iter()
                .filter(|(agent, _)| skill::agent_reads_skill(agent, variant))
                .map(|(agent, _)| agent.clone())
                .collect();
            (variant.id.clone(), readers)
        })
        .collect();
    let mut families = skill::families(found.skills);
    for family in &mut families {
        let evidence: Vec<_> = held
            .usage
            .iter()
            .filter(|event| matches_family(event, family))
            .collect();
        family.usage = Usage {
            last_used_ms: evidence.iter().map(|e| e.at_ms).max(),
            count_7d: evidence.len(),
            agents: evidence
                .iter()
                .map(|e| e.agent.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
    }
    Report {
        families,
        installed_agents_by_skill,
        external_agents,
        external_unavailable,
        sources: found.sources,
        agents,
        required,
        policy: context.policy,
        reads,
        capped,
        evidence_count: held.usage.len(),
        evidence_scope: if ZoIndexAdapter.available() {
            "hooks-and-zo"
        } else {
            "hooks-only"
        },
        retained_evidence_bytes: held.usage.iter().map(Evidence::retained_bytes).sum(),
        plans: held.plans.iter().cloned().collect(),
    }
}
fn matches_family(event: &Evidence, family: &SkillFamily) -> bool {
    event.name.eq_ignore_ascii_case(&family.name)
        || event
            .path
            .as_ref()
            .is_some_and(|path| family.variants.iter().any(|skill| &skill.file == path))
}
pub(crate) fn installed_agents() -> Vec<(String, String)> {
    detected_agents(false)
        .into_iter()
        .filter(|agent| agent.installed)
        .map(|agent| (agent.id.to_string(), agent.name.to_string()))
        .collect()
}
pub(crate) fn legacy_list(context: Context) -> SkillReport {
    runtime().scan(context.sources(), &context.policy, false).0
}
pub(crate) fn legacy_orchestration(context: Context) -> skill::OrchestrationReport {
    skill::orchestration_report(&legacy_list(context).skills, &installed_agents())
}
pub(crate) fn legacy_computer_use(context: Context) -> skill::ComputerUseSkillReport {
    skill::computer_use_skill_report(&legacy_list(context).skills, &installed_agents())
}
pub(crate) fn install_bundled(
    context: Context,
    name: String,
) -> Result<Vec<zerocode_core::skill_install::SkillInstallOutcome>, String> {
    install_bundled_skill(&name, &context.home, &installed_agents())
}
pub(crate) fn install_bundled_skill(
    name: &str,
    home: &Path,
    agents: &[(String, String)],
) -> Result<Vec<zerocode_core::skill_install::SkillInstallOutcome>, String> {
    let answer = zerocode_core::skill_install::install_bundled_skill(name, home, agents)?;
    runtime().roots.clear();
    Ok(answer)
}

fn read_bounded(path: &Path, limit: usize) -> Result<String, String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > limit {
        return Err("skills.fileTooLarge".into());
    }
    Ok(text)
}
pub(crate) fn checked_file(context: &Context, path: &Path) -> Result<PathBuf, String> {
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if !real.is_file()
        || !context.sources().iter().any(|source| {
            source
                .path
                .canonicalize()
                .is_ok_and(|root| real.starts_with(root))
        })
    {
        return Err("skills.outsideRoot".into());
    }
    Ok(real)
}
#[derive(Serialize)]
pub(crate) struct Detail {
    pub skill: Skill,
    pub markdown: String,
    pub evidence: Vec<Evidence>,
}
pub(crate) fn detail(context: Context, id: String) -> Result<Detail, String> {
    let found = legacy_list(context.clone());
    let skill = found
        .skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("skills.notFound")?;
    let path = checked_file(&context, &skill.file)?;
    let markdown = read_bounded(&path, context.policy.max_detail_bytes)?;
    let family = skill::families(vec![skill.clone()]).remove(0);
    let evidence = runtime()
        .usage
        .iter()
        .filter(|e| matches_family(e, &family))
        .cloned()
        .collect();
    Ok(Detail {
        skill,
        markdown,
        evidence,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub at_ms: i64,
    pub status: String,
    pub names: Vec<String>,
    pub error: Option<String>,
    before: Vec<(PathBuf, String)>,
}
fn installation_state(found: &[Skill], names: &[String]) -> Vec<(PathBuf, String)> {
    let mut state: Vec<_> = found
        .iter()
        .filter(|s| names.iter().any(|name| s.name.eq_ignore_ascii_case(name)))
        .map(|s| (s.file.clone(), s.digest.clone()))
        .collect();
    state.sort();
    state
}
pub(crate) fn terminal_output(state: &AppState, term: TermId, context: &Context) -> Option<String> {
    let handle = state.terminals().handle(term)?;
    let mut pty = lock_pty(&handle);
    let grid = pty.terminal_mut().grid_mut();
    let mut lines = vec![grid.visible_text()];
    let mut bytes = lines[0].len();
    for index in (0..grid.scrollback_len())
        .rev()
        .take(context.policy.max_entries_per_root)
    {
        let line = grid.scrollback_line(index);
        if bytes + line.len() >= context.policy.max_evidence_bytes {
            break;
        }
        bytes += line.len();
        lines.push(line);
    }
    lines.reverse();
    Some(lines.join("\n"))
}
pub(crate) fn rescan(context: Context, output: Option<String>) -> Report {
    let mut report = list(context.clone(), true);
    let found: Vec<_> = report
        .families
        .iter()
        .flat_map(|family| family.variants.iter().cloned())
        .collect();
    let mut held = runtime();
    if let Some(plan) = held.plans.back_mut() {
        let current = installation_state(&found, &plan.names);
        plan.error = output
            .as_deref()
            // A fresh shell belongs to this plan. Long commands wrap in the
            // grid, so an absent contiguous echo must not hide its error line.
            .map(|output| {
                output
                    .rsplit_once(&plan.command)
                    .map_or(output, |(_, tail)| tail)
            })
            .and_then(|tail| skill::parse_bundle_output(tail, &context.policy).error);
        plan.status = if plan.error.is_some() {
            "failed"
        } else if current != plan.before {
            "changed"
        } else {
            "unchanged"
        }
        .into();
        if let Err(error) = save_json(&context.data.join(PLANS_FILE), &held.plans) {
            eprintln!("skills plan: {error}");
        }
    }
    report.plans = held.plans.iter().cloned().collect();
    report
}
pub(crate) fn plan(
    context: Context,
    repository: Option<String>,
    names: Vec<String>,
    agents: Vec<String>,
    action: String,
) -> Result<Plan, String> {
    if names.len() > context.policy.max_bundle_names || agents.len() > context.policy.max_roots {
        return Err("skills.invalidPlan".into());
    }
    let repository = repository
        .as_deref()
        .unwrap_or(skill::SKILLS_REPOSITORY_URL);
    let name_refs: Vec<_> = names.iter().map(String::as_str).collect();
    let agent_refs: Vec<_> = agents.iter().map(String::as_str).collect();
    let command = match action.as_str() {
        "list" => skill::bundle_list_command(repository),
        "update" if names.len() == 1 => skill::skill_update_command(&names[0]),
        "install" if !agents.is_empty() => {
            skill::skill_install_command_from(repository, &name_refs, &agent_refs)
        }
        _ => None,
    }
    .ok_or("skills.invalidPlan")?;
    let before = installation_state(&legacy_list(context.clone()).skills, &names);
    let plan = Plan {
        names,
        before,
        error: None,
        command,
        rows: context.policy.terminal_rows,
        cols: context.policy.terminal_cols,
        at_ms: now_epoch_ms(),
        status: "prepared".into(),
    };
    if serde_json::to_vec(&plan).map_err(|e| e.to_string())?.len() > context.policy.max_plan_bytes {
        return Err("skills.invalidPlan".into());
    }
    let mut held = runtime();
    held.plans.push_back(plan.clone());
    while held.plans.len() > context.policy.max_plans
        || serde_json::to_vec(&held.plans)
            .map_err(|e| e.to_string())?
            .len()
            > context.policy.max_plan_bytes
    {
        held.plans.pop_front();
    }
    // Preparing a command is recorded as prepared; it is never execution proof.
    save_json(&context.data.join(PLANS_FILE), &held.plans)?;
    Ok(plan)
}
fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let data = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    durable_file::replace_bytes(path, &data)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Called once by the accepted hook road, before its short activity ring drops
/// older rows. No skill matching or persistence logic lives in the pane module.
pub(crate) fn note_hook(
    state: &AppState,
    envelope: &zerocode_core::hook::HookEnvelope,
    activity: &zerocode_core::hook::Activity,
) {
    let tool = activity.verb.as_str();
    if tool != "read" && !tool.eq_ignore_ascii_case("skill") {
        return;
    }
    if !matches!(
        activity.phase,
        zerocode_core::hook::Phase::Started | zerocode_core::hook::Phase::Finished
    ) {
        return;
    }
    let Some(mut evidence) = hook_evidence(envelope, now_epoch_ms()) else {
        return;
    };
    let Ok(context) = Context::from_state(state) else {
        return;
    };
    if let Some(path) = &evidence.path
        && checked_file(&context, path).is_err()
    {
        return;
    }
    let data = &context.data;
    evidence.source = data.join(USAGE_FILE).to_string_lossy().into_owned();
    let policy = context.policy;
    let mut held = runtime();
    if held.loaded_data.as_deref() != Some(data) {
        held.ingest(
            HookRingAdapter(&data.join(USAGE_FILE)).read(&policy),
            &policy,
            now_epoch_ms(),
        );
        held.loaded_data = Some(data.to_path_buf());
    }
    held.ingest([evidence], &policy, now_epoch_ms());
    if let Err(error) = save_json(&data.join(USAGE_FILE), &held.usage) {
        eprintln!("skills usage: {error}");
    }
}
fn hook_evidence(envelope: &zerocode_core::hook::HookEnvelope, now: i64) -> Option<Evidence> {
    let value: serde_json::Value = serde_json::from_str(&envelope.payload).ok()?;
    let tool = value
        .get("tool_name")
        .or_else(|| value.get("name"))?
        .as_str()?;
    let input = value.get("tool_input").or_else(|| value.get("input"))?;
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    let name = if tool.eq_ignore_ascii_case("skill") {
        input.get("skill")?.as_str()?.to_string()
    } else if tool.eq_ignore_ascii_case("read") || tool.eq_ignore_ascii_case("read_file") {
        let file = path.as_ref()?;
        if file.file_name()? != skill::SKILL_FILE {
            return None;
        }
        file.parent()?.file_name()?.to_string_lossy().into_owned()
    } else {
        return None;
    };
    let call = value
        .get("tool_use_id")
        .or_else(|| value.get("call_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| now.to_string());
    Some(Evidence {
        id: skill::path_id(Path::new(&format!(
            "{}:{}:{call}",
            envelope.pane_key, envelope.launch_token
        ))),
        name,
        path,
        agent: envelope.agent.slug().into(),
        pane: envelope.pane_key.clone(),
        at_ms: now,
        source: String::new(),
    })
}

pub(crate) fn parse_bundle(context: Context, output: &str) -> skill::BundleListing {
    skill::parse_bundle_output(output, &context.policy)
}
pub(crate) fn reveal_skill(context: Context, path: String) -> Result<(), String> {
    let real = checked_file(&context, Path::new(&path))?;
    #[cfg(target_os = "macos")]
    let (launcher, args): (&str, Vec<&str>) = ("open", vec!["-R"]);
    #[cfg(target_os = "linux")]
    let (launcher, args): (&str, Vec<&str>) = ("xdg-open", Vec::new());
    #[cfg(target_os = "windows")]
    let (launcher, args): (&str, Vec<&str>) = ("explorer", vec!["/select,"]);
    // On linux there is no portable "reveal", so the containing directory opens —
    // which shows the file, without deciding what opens the file itself.
    #[cfg(target_os = "linux")]
    let shown = real.parent().unwrap_or(&real).to_path_buf();
    #[cfg(not(target_os = "linux"))]
    let shown = real;
    let mut opener = crate::proc::quiet_command(launcher);
    opener.args(args).arg(&shown);
    // The same reaping door as `open_url`, for the same zombie.
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_adapters_count_hooks_not_availability_and_bound_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let policy = Policy::overlay(&json!({"max_usage_events":1,"usage_window_ms":100}));
        assert!(!ZoIndexAdapter.available());
        assert!(ZoIndexAdapter.read(&policy).is_empty());
        let rows = vec![Evidence {
            id: "call-1".into(),
            name: "loaded".into(),
            path: None,
            agent: "claude".into(),
            pane: "term-1".into(),
            at_ms: 100,
            source: "hooks".into(),
        }];
        let ring = dir.path().join("ring.json");
        save_json(&ring, &rows).unwrap();
        let mut runtime = Runtime::default();
        runtime.ingest(HookRingAdapter(&ring).read(&policy), &policy, 150);
        runtime.ingest(rows, &policy, 150);
        assert_eq!(runtime.usage.len(), 1);
        runtime.ingest([], &policy, 250);
        assert!(runtime.usage.is_empty());
    }
    #[test]
    fn hook_skill_and_read_evidence_keep_call_and_pane_identity() {
        let envelope: zerocode_core::hook::HookEnvelope = serde_json::from_value(json!({"agent":"claude","pane_key":"term-8","payload":r#"{"tool_name":"Skill","tool_use_id":"call-1","tool_input":{"skill":"review"}}"#})).unwrap();
        let row = hook_evidence(&envelope, 10).unwrap();
        assert_eq!(row.name, "review");
        assert_eq!(row.pane, "term-8");
        let mut read = envelope.clone();
        read.payload =
            r#"{"tool_name":"Read","tool_input":{"file_path":"/skills/design/SKILL.md"}}"#.into();
        assert_eq!(hook_evidence(&read, 11).unwrap().name, "design");
        read.payload =
            r#"{"tool_name":"Read","tool_input":{"file_path":"/skills/design/readme.md"}}"#.into();
        assert!(hook_evidence(&read, 12).is_none());
    }
    #[test]
    fn runtime_cache_rescans_after_install_and_keeps_only_active_roots() {
        let temp = tempfile::tempdir().unwrap();
        let source = SkillSource {
            id: "home-codex".into(),
            label: "Codex".into(),
            path: temp.path().join("skills"),
            kind: SourceKind::Home,
            providers: vec!["codex".into()],
            owner: Some("codex".into()),
        };
        let policy = Policy::default();
        let mut held = Runtime::default();
        let (empty, reads, _) = held.scan(vec![source.clone()], &policy, false);
        assert!(empty.skills.is_empty());
        assert_eq!(reads, 0);
        let dir = source.path.join("review");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: review\nversion: 1.0.0\n---\nReview it.",
        )
        .unwrap();
        let (installed, reads, _) = held.scan(vec![source.clone()], &policy, true);
        assert_eq!(reads, 1);
        assert_ne!(
            installation_state(&empty.skills, &["review".into()]),
            installation_state(&installed.skills, &["review".into()])
        );
        assert_eq!(held.scan(vec![source], &policy, false).1, 0);
        held.scan(Vec::new(), &policy, false);
        assert!(held.roots.is_empty());
    }
    #[test]
    fn evidence_ring_enforces_byte_and_count_caps_and_ignores_future_events() {
        let policy = Policy::overlay(&json!({"max_usage_events":2,"max_evidence_bytes":1024}));
        let mut held = Runtime::default();
        for i in 0..100 {
            held.ingest(
                [Evidence {
                    id: i.to_string(),
                    name: "review".into(),
                    path: None,
                    agent: "claude".into(),
                    pane: "term-1".into(),
                    at_ms: 100,
                    source: "hook-ring".into(),
                }],
                &policy,
                100,
            );
        }
        assert_eq!(held.usage.len(), 2);
        assert!(
            held.usage
                .iter()
                .map(Evidence::retained_bytes)
                .sum::<usize>()
                <= policy.max_evidence_bytes
        );
        let future = Evidence {
            id: "future".into(),
            name: "review".into(),
            path: None,
            agent: "claude".into(),
            pane: "term-1".into(),
            at_ms: 101,
            source: "hooks".into(),
        };
        held.ingest([future], &policy, 100);
        assert!(!held.usage.iter().any(|e| e.id == "future"));
    }
    #[test]
    fn detail_and_read_evidence_reject_files_outside_discovery_roots() {
        let temp = tempfile::tempdir().unwrap();
        let context = Context {
            home: temp.path().into(),
            repos: Vec::new(),
            data: temp.path().into(),
            policy: Policy::default(),
        };
        let outside = temp.path().join("SKILL.md");
        std::fs::write(&outside, "# Outside").unwrap();
        assert!(checked_file(&context, &outside).is_err());
        let inside = temp.path().join(".claude/skills/review/SKILL.md");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, "# Review").unwrap();
        assert!(checked_file(&context, &inside).is_ok());
        #[cfg(unix)]
        {
            let link = inside.parent().unwrap().join("outside.md");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(checked_file(&context, &link).is_err());
        }
    }
}
