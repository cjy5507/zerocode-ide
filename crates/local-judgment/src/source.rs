use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Digest as Sha2Digest, Sha256};

use crate::catalog::{self, Artifact, Catalog, EngineKind, Model};
use crate::{Error, Result, adapter, storage};

pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_JSON_TOTAL: usize = 8 * 1024 * 1024;
pub const MAX_DATA_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SourceInput {
    HuggingFace {
        repository: String,
        revision: Option<String>,
        subfolder: Option<String>,
    },
    LocalDirectory {
        directory: String,
    },
    LocalFile {
        file: String,
    },
    HuggingFaceFile {
        url: String,
    },
}

#[derive(Clone)]
pub(crate) struct Inspected {
    pub kind: &'static str,
    pub input_template: Option<String>,
    pub model: Model,
    pub directory: Option<PathBuf>,
    stamps: Vec<Stamp>,
}

#[derive(Clone)]
struct Stamp {
    path: String,
    bytes: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HubSource {
    pub repository: String,
    pub revision: String,
    pub subfolder: Option<String>,
}

#[derive(Deserialize)]
struct HubMetadata {
    sha: String,
    siblings: Vec<HubFile>,
    #[serde(rename = "cardData")]
    card: Option<Value>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct HubFile {
    rfilename: String,
    #[serde(rename = "blobId")]
    blob_id: Option<String>,
    size: Option<u64>,
    lfs: Option<HubLfs>,
}

#[derive(Deserialize)]
struct HubLfs {
    sha256: String,
    size: u64,
}

pub(crate) type Cancelled = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) type Progress = Arc<dyn Fn(u64) + Send + Sync>;

fn check(cancelled: &Cancelled) -> Result<()> {
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(crate) fn hub_source(
    repository: &str,
    revision: Option<&str>,
    subfolder: Option<&str>,
) -> Result<HubSource> {
    let lowercase = repository.to_ascii_lowercase();
    if repository.split('/').any(|part| matches!(part, "." | ".."))
        || repository.contains('\\')
        || lowercase.contains("%2e")
        || lowercase.contains("%2f")
        || lowercase.contains("%5c")
    {
        return Err(Error::SourceInvalid);
    }
    let mut parts: Vec<String> = if repository.starts_with("https://") {
        let address = url::Url::parse(repository).map_err(|_| Error::SourceInvalid)?;
        if address.host_str() != Some("huggingface.co")
            || address.port().is_some()
            || !catalog::https(repository)
            || address.query().is_some()
            || address.fragment().is_some()
            || address.path().contains("//")
        {
            return Err(Error::SourceInvalid);
        }
        address
            .path_segments()
            .ok_or(Error::SourceInvalid)?
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        repository.split('/').map(str::to_owned).collect()
    };
    if parts.len() < 2 || !segment(&parts[0]) || !segment(&parts[1]) {
        return Err(Error::SourceInvalid);
    }
    let repository = format!("{}/{}", parts[0], parts[1]);
    let suffix = parts.split_off(2);
    let (url_revision, url_subfolder) = if suffix.is_empty() {
        (None, None)
    } else if suffix.len() >= 2 && suffix[0] == "tree" && segment(&suffix[1]) {
        (
            Some(suffix[1].clone()),
            (suffix.len() > 2).then(|| suffix[2..].join("/")),
        )
    } else {
        return Err(Error::SourceInvalid);
    };
    if revision.is_some_and(|value| url_revision.as_deref().is_some_and(|given| given != value))
        || subfolder
            .is_some_and(|value| url_subfolder.as_deref().is_some_and(|given| given != value))
    {
        return Err(Error::SourceInvalid);
    }
    let revision = revision
        .map(str::to_owned)
        .or(url_revision)
        .unwrap_or_else(|| "main".to_owned());
    if !segment(&revision) {
        return Err(Error::SourceInvalid);
    }
    let subfolder = subfolder
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or(url_subfolder);
    if let Some(subfolder) = &subfolder {
        catalog::relative(subfolder)?;
    }
    Ok(HubSource {
        repository,
        revision,
        subfolder,
    })
}

fn hub_url(
    source: &HubSource,
    revision: &str,
    action: &str,
    file: Option<&str>,
) -> Result<url::Url> {
    let mut address =
        url::Url::parse("https://huggingface.co").map_err(|_| Error::SourceInvalid)?;
    {
        let mut path = address
            .path_segments_mut()
            .map_err(|_| Error::SourceInvalid)?;
        if action == "api" {
            path.extend(["api", "models"]);
        }
        path.extend(source.repository.split('/'));
        if action == "api" {
            path.push("revision").push(revision);
        } else {
            path.push(action).push(revision);
            if let Some(subfolder) = &source.subfolder {
                path.extend(subfolder.split('/'));
            }
            if let Some(file) = file {
                path.extend(file.split('/'));
            }
        }
    }
    if action == "api" {
        address.query_pairs_mut().append_pair("blobs", "true");
    }
    Ok(address)
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 4
                || attempt.url().scheme() != "https"
                || attempt.url().host_str() != Some("huggingface.co")
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| Error::SourceUnavailable)
}

async fn fetch(
    client: &reqwest::Client,
    address: url::Url,
    limit: usize,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Vec<u8>> {
    check(cancelled)?;
    let mut response = client
        .get(address)
        .send()
        .await
        .map_err(|_| Error::SourceUnavailable)?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(Error::SourceAuthRequired);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(Error::SourceUnavailable);
    }
    if response
        .content_length()
        .is_some_and(|bytes| bytes > limit as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::SourceUnavailable)?
    {
        check(cancelled)?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::TooLarge);
        }
        progress(chunk.len() as u64);
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn hub_json(
    client: &reqwest::Client,
    source: &HubSource,
    revision: &str,
    file: &HubFile,
    budget: &mut usize,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Vec<u8>> {
    let size = file.size.ok_or(Error::SourceHashUnavailable)?;
    let size = usize::try_from(size).map_err(|_| Error::TooLarge)?;
    if size > MAX_JSON_BYTES || size > *budget {
        return Err(Error::TooLarge);
    }
    let prefix = source
        .subfolder
        .as_ref()
        .map(|folder| format!("{folder}/"))
        .unwrap_or_default();
    let path = file
        .rfilename
        .strip_prefix(&prefix)
        .ok_or(Error::SourceInvalid)?;
    catalog::relative(path)?;
    let bytes = fetch(
        client,
        hub_url(source, revision, "resolve", Some(path))?,
        size,
        cancelled,
        progress,
    )
    .await?;
    verify_hub_json(file, &bytes)?;
    *budget -= bytes.len();
    Ok(bytes)
}

fn verify_hub_json(file: &HubFile, bytes: &[u8]) -> Result<()> {
    if file.size != Some(bytes.len() as u64) {
        return Err(Error::Integrity);
    }
    if let Some(lfs) = &file.lfs {
        if lfs.size != bytes.len() as u64 || format!("{:x}", Sha256::digest(bytes)) != lfs.sha256 {
            return Err(Error::Integrity);
        }
    } else {
        let expected = file
            .blob_id
            .as_ref()
            .filter(|digest| hex(digest, 40))
            .ok_or(Error::SourceHashUnavailable)?;
        if git_blob_digest(bytes) != *expected {
            return Err(Error::Integrity);
        }
    }
    serde_json::from_slice::<Value>(bytes).map_err(|_| Error::Manifest)?;
    Ok(())
}

fn git_blob_digest(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    let mut encoded = String::with_capacity(40);
    for byte in digest.finalize() {
        encoded.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        encoded.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
    }
    encoded
}

pub(crate) fn schema(configs: &BTreeMap<String, Value>) -> Result<(EngineKind, Option<String>)> {
    if let Some(agent) = configs.get("rl_agent_config.json") {
        if !agent.get("encoder").is_some_and(Value::is_string)
            || !agent
                .get("head_layers")
                .and_then(Value::as_u64)
                .is_some_and(|layers| (1..=8).contains(&layers))
            || !agent
                .get("max_len")
                .and_then(Value::as_u64)
                .is_some_and(|tokens| (1..=8192).contains(&tokens))
        {
            return Err(Error::Manifest);
        }
        for name in [
            "rl_agent_config.json",
            "encoder/config.json",
            "tokenizer/tokenizer_config.json",
        ] {
            if let Some(value) = configs.get(name) {
                adapter::reject_code(value, EngineKind::Laya)?;
            }
        }
        return Ok((EngineKind::Laya, None));
    }
    if let Some(decision) = configs.get("decision_config.json") {
        if decision.get("format_version").and_then(Value::as_u64) != Some(1)
            || decision.get("format_id").and_then(Value::as_str) != Some("d3-code-readout-v1")
        {
            return Err(Error::ModelFormatUnsupported);
        }
        let kind = configs
            .get("config.json")
            .and_then(|config| config.get("model_type"))
            .and_then(Value::as_str);
        if !matches!(kind, Some("qwen3_5" | "qwen3_vl")) {
            return Err(Error::ModelFormatUnsupported);
        }
        for value in configs.values() {
            adapter::reject_code(value, EngineKind::TransformersSystemOne)?;
        }
        return Ok((
            EngineKind::TransformersSystemOne,
            Some("d3-code-readout-v1".to_owned()),
        ));
    }
    Err(Error::SourceNotNative)
}

fn data_files(engine: EngineKind, configs: &BTreeMap<String, Value>) -> Result<Vec<String>> {
    let mut files: Vec<String> = match engine {
        EngineKind::Laya => [
            "model.safetensors",
            "rl_agent_config.json",
            "encoder/config.json",
            "tokenizer/tokenizer_config.json",
            "tokenizer/tokenizer.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        EngineKind::TransformersSystemOne => [
            "config.json",
            "decision_config.json",
            "readout.safetensors",
            "tokenizer.json",
            "tokenizer_config.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        EngineKind::LlamaCpp | EngineKind::SystemOneServer => return Err(Error::EngineUnsupported),
    };
    if engine == EngineKind::TransformersSystemOne {
        if let Some(index) = configs.get("model.safetensors.index.json") {
            let weights = index
                .get("weight_map")
                .and_then(Value::as_object)
                .ok_or(Error::Manifest)?;
            let mut shards = BTreeSet::new();
            for value in weights.values() {
                let name = value.as_str().ok_or(Error::Manifest)?;
                catalog::relative(name)?;
                if !name.ends_with(".safetensors") {
                    return Err(Error::ExecutableArtifact);
                }
                shards.insert(name.to_owned());
            }
            if shards.is_empty() || shards.len() > 120 {
                return Err(Error::TooLarge);
            }
            files.push("model.safetensors.index.json".to_owned());
            files.extend(shards);
        } else {
            files.push("model.safetensors".to_owned());
        }
    }
    Ok(files)
}

struct Description {
    engine: EngineKind,
    native_format: Option<String>,
    revision: Option<String>,
    source: Option<String>,
    label: String,
    server_model_id: Option<String>,
    license: String,
}

fn model(mut files: Vec<Artifact>, description: Description) -> Result<Model> {
    let Description {
        engine,
        native_format,
        revision,
        source,
        label,
        server_model_id,
        license,
    } = description;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let identities: Vec<_> = files
        .iter()
        .map(|file| (&file.path, file.bytes, &file.sha256))
        .collect();
    let bytes =
        serde_json::to_vec(&(engine, &native_format, &identities)).map_err(|_| Error::Manifest)?;
    let fingerprint = format!("{:x}", Sha256::digest(bytes));
    let local = source.is_none();
    let source = source.unwrap_or_else(|| format!("local://{fingerprint}"));
    let revision = revision.unwrap_or_else(|| fingerprint[..40].to_owned());
    if local {
        for file in &mut files {
            file.url = format!("local://{fingerprint}/{}", file.path);
        }
    }
    let identity = format!(
        "{:x}",
        Sha256::digest(format!("{source}:{revision}:{fingerprint}"))
    );
    let model = Model {
        id: format!("native-{}", &identity[..24]),
        label,
        engine,
        protocol: if engine == EngineKind::LlamaCpp {
            judgment_provider::Protocol::OpenaiJson
        } else {
            judgment_provider::Protocol::SystemOne
        },
        revision,
        license,
        source,
        quality: "source_inspected_unverified".to_owned(),
        files,
        server_model_id,
        weights_bytes: None,
        native_format,
    };
    Catalog {
        version: 1,
        models: vec![model.clone()],
    }
    .validate()?;
    Ok(model)
}

pub(crate) async fn inspect(
    input: SourceInput,
    cancelled: Cancelled,
    progress: Progress,
) -> Result<Inspected> {
    match input {
        SourceInput::LocalFile { file } => {
            if file.len() > 4096 {
                return Err(Error::SourceInvalid);
            }
            tokio::task::spawn_blocking(move || {
                inspect_local_gguf(Path::new(&file), &cancelled, &progress)
            })
            .await
            .map_err(|_| Error::Storage)?
        }
        SourceInput::HuggingFaceFile { url } => inspect_hub_gguf(&url, &cancelled, &progress).await,
        SourceInput::HuggingFace {
            repository,
            revision,
            subfolder,
        } => {
            inspect_hub(
                hub_source(&repository, revision.as_deref(), subfolder.as_deref())?,
                &cancelled,
                &progress,
            )
            .await
        }
        SourceInput::LocalDirectory { directory } => {
            if directory.len() > 4096 {
                return Err(Error::SourceInvalid);
            }
            tokio::task::spawn_blocking(move || {
                inspect_local(Path::new(&directory), &cancelled, &progress)
            })
            .await
            .map_err(|_| Error::Storage)?
        }
    }
}

async fn inspect_hub(
    source: HubSource,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Inspected> {
    let client = client()?;
    let bytes = fetch(
        &client,
        hub_url(&source, &source.revision, "api", None)?,
        MAX_METADATA_BYTES,
        cancelled,
        progress,
    )
    .await?;
    let metadata: HubMetadata =
        serde_json::from_slice(&bytes).map_err(|_| Error::SourceUnavailable)?;
    if !hex(&metadata.sha, 40) || metadata.siblings.len() > 2048 {
        return Err(Error::Manifest);
    }
    let prefix = source
        .subfolder
        .as_ref()
        .map(|folder| format!("{folder}/"))
        .unwrap_or_default();
    let mut entries = BTreeMap::new();
    for file in &metadata.siblings {
        if let Some(path) = file.rfilename.strip_prefix(&prefix)
            && entries.insert(path.to_owned(), file).is_some()
        {
            return Err(Error::Manifest);
        }
    }
    let mut budget = MAX_JSON_TOTAL;
    let mut configs = BTreeMap::new();
    let mut inspected = BTreeMap::new();
    let names = configuration_names(
        entries.contains_key("rl_agent_config.json"),
        entries.contains_key("decision_config.json"),
    )
    .map_err(|error| {
        if error == Error::SourceNotNative
            && metadata
                .tags
                .iter()
                .any(|tag| matches!(tag.as_str(), "decision-model" | "system-one"))
        {
            Error::ModelFormatUnsupported
        } else {
            error
        }
    })?;
    for name in names {
        if let Some(file) = entries.get(name) {
            let bytes = hub_json(
                &client,
                &source,
                &metadata.sha,
                file,
                &mut budget,
                cancelled,
                progress,
            )
            .await?;
            configs.insert(
                name.to_owned(),
                serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)?,
            );
            inspected.insert(name.to_owned(), bytes);
        }
    }
    let (engine, format) = schema(&configs)?;
    let mut artifacts = Vec::new();
    for name in data_files(engine, &configs)? {
        check(cancelled)?;
        let file = entries.get(&name).ok_or(Error::ModelManifestMissing)?;
        let size = file.size.ok_or(Error::SourceHashUnavailable)?;
        let sha256 = if let Some(lfs) = &file.lfs {
            if lfs.size != size || !hex(&lfs.sha256, 64) {
                return Err(Error::Manifest);
            }
            lfs.sha256.clone()
        } else {
            if !name.ends_with(".json") {
                return Err(Error::SourceHashUnavailable);
            }
            let bytes = match inspected.remove(&name) {
                Some(bytes) => bytes,
                None => {
                    hub_json(
                        &client,
                        &source,
                        &metadata.sha,
                        file,
                        &mut budget,
                        cancelled,
                        progress,
                    )
                    .await?
                }
            };
            format!("{:x}", Sha256::digest(&bytes))
        };
        artifacts.push(Artifact {
            path: name.clone(),
            bytes: size,
            sha256,
            url: hub_url(&source, &metadata.sha, "resolve", Some(&name))?.to_string(),
            package: None,
            version: None,
            license: None,
        });
    }
    let label = format!(
        "{}{}",
        source.repository,
        source
            .subfolder
            .as_ref()
            .map(|folder| format!(" / {folder}"))
            .unwrap_or_default()
    );
    let label: String = label
        .char_indices()
        .take_while(|(offset, character)| offset + character.len_utf8() <= 160)
        .map(|(_, character)| character)
        .collect();
    let license = metadata
        .card
        .as_ref()
        .and_then(|card| card.get("license"))
        .and_then(Value::as_str)
        .unwrap_or("License metadata unavailable")
        .to_owned();
    let source_url = hub_url(&source, &metadata.sha, "tree", None)?.to_string();
    Ok(Inspected {
        kind: "hugging_face",
        input_template: None,
        model: model(
            artifacts,
            Description {
                engine,
                native_format: format,
                revision: Some(metadata.sha),
                source: Some(source_url),
                label,
                server_model_id: Some(source.repository),
                license,
            },
        )?,
        directory: None,
        stamps: Vec::new(),
    })
}

pub(crate) fn inspect_local(
    directory: &Path,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Inspected> {
    storage::plain(directory)?;
    let root = fs::canonicalize(directory).map_err(|_| Error::Storage)?;
    if !root.is_dir() {
        return Err(Error::SourceInvalid);
    }
    let mut configs = BTreeMap::new();
    let mut config_hashes = BTreeMap::new();
    let mut budget = MAX_JSON_TOTAL;
    let laya = storage::child(&root, "rl_agent_config.json")?.exists();
    let decision = storage::child(&root, "decision_config.json")?.exists();
    for name in configuration_names(laya, decision)? {
        check(cancelled)?;
        let path = storage::child(&root, name)?;
        if path.exists() {
            let bytes = storage::bounded_read(&path, MAX_JSON_BYTES.min(budget))?;
            budget -= bytes.len();
            configs.insert(
                name.to_owned(),
                serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)?,
            );
            config_hashes.insert(name.to_owned(), format!("{:x}", Sha256::digest(&bytes)));
        }
    }
    let (engine, format) = schema(&configs)?;
    let mut artifacts = Vec::new();
    let mut stamps = Vec::new();
    let mut total = 0u64;
    for name in data_files(engine, &configs)? {
        check(cancelled)?;
        let path = storage::child(&root, &name)?;
        if !path.exists() {
            return Err(Error::ModelManifestMissing);
        }
        storage::plain(&path)?;
        let mut file = File::open(&path).map_err(|_| Error::Storage)?;
        let before = file.metadata().map_err(|_| Error::Storage)?;
        if !before.is_file() {
            return Err(Error::UnsafePath);
        }
        total = total.checked_add(before.len()).ok_or(Error::TooLarge)?;
        if total > MAX_DATA_BYTES {
            return Err(Error::TooLarge);
        }
        if name.ends_with(".safetensors") {
            safetensors_header(&mut file, before.len())?;
            file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
        }
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut received = 0u64;
        loop {
            check(cancelled)?;
            let count = file.read(&mut buffer).map_err(|_| Error::Storage)?;
            if count == 0 {
                break;
            }
            received = received.checked_add(count as u64).ok_or(Error::TooLarge)?;
            if received > before.len() {
                return Err(Error::SourceChanged);
            }
            digest.update(&buffer[..count]);
            progress(count as u64);
        }
        let after = file.metadata().map_err(|_| Error::Storage)?;
        if received != before.len()
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(Error::SourceChanged);
        }
        let sha256 = format!("{:x}", digest.finalize());
        if config_hashes
            .get(&name)
            .is_some_and(|expected| *expected != sha256)
        {
            return Err(Error::SourceChanged);
        }
        stamps.push(Stamp {
            path: name.clone(),
            bytes: before.len(),
            modified: before.modified().ok(),
        });
        artifacts.push(Artifact {
            path: name,
            bytes: before.len(),
            sha256,
            url: String::new(),
            package: None,
            version: None,
            license: None,
        });
    }
    Ok(Inspected {
        kind: "local_directory",
        input_template: None,
        model: model(
            artifacts,
            Description {
                engine,
                native_format: format,
                revision: None,
                source: None,
                label: "Local native decision model".to_owned(),
                server_model_id: None,
                license: "License not inspected locally".to_owned(),
            },
        )?,
        directory: Some(root),
        stamps,
    })
}

fn configuration_names(laya: bool, decision: bool) -> Result<Vec<&'static str>> {
    match (laya, decision) {
        (true, false) => Ok(vec![
            "rl_agent_config.json",
            "encoder/config.json",
            "tokenizer/tokenizer_config.json",
        ]),
        (false, true) => Ok(vec![
            "decision_config.json",
            "config.json",
            "tokenizer_config.json",
            "model.safetensors.index.json",
        ]),
        (false, false) => Err(Error::SourceNotNative),
        (true, true) => Err(Error::Manifest),
    }
}

fn safetensors_header(file: &mut File, bytes: u64) -> Result<()> {
    let mut prefix = [0u8; 8];
    file.read_exact(&mut prefix)
        .map_err(|_| Error::WeightsFormat)?;
    let length = u64::from_le_bytes(prefix);
    if length == 0 || length > 8 * 1024 * 1024 || length > bytes.saturating_sub(8) {
        return Err(Error::WeightsFormat);
    }
    let mut header = vec![0u8; usize::try_from(length).map_err(|_| Error::WeightsFormat)?];
    file.read_exact(&mut header)
        .map_err(|_| Error::WeightsFormat)?;
    let value: Value = serde_json::from_slice(&header).map_err(|_| Error::WeightsFormat)?;
    let tensors = value.as_object().ok_or(Error::WeightsFormat)?;
    let mut ranges = Vec::new();
    for (name, tensor) in tensors {
        if name == "__metadata__" {
            if !tensor
                .as_object()
                .is_some_and(|fields| fields.values().all(Value::is_string))
            {
                return Err(Error::WeightsFormat);
            }
            continue;
        }
        let width = match tensor.get("dtype").and_then(Value::as_str) {
            Some("F64" | "I64" | "U64") => 8u64,
            Some("F32" | "I32" | "U32") => 4,
            Some("F16" | "BF16" | "I16" | "U16") => 2,
            Some("I8" | "U8" | "BOOL") => 1,
            _ => return Err(Error::ModelFormatUnsupported),
        };
        let shape = tensor
            .get("shape")
            .and_then(Value::as_array)
            .ok_or(Error::WeightsFormat)?;
        if shape.len() > 8 {
            return Err(Error::WeightsFormat);
        }
        let mut expected = width;
        for dimension in shape {
            expected = expected
                .checked_mul(dimension.as_u64().ok_or(Error::WeightsFormat)?)
                .ok_or(Error::WeightsFormat)?;
        }
        let offsets = tensor
            .get("data_offsets")
            .and_then(Value::as_array)
            .filter(|offsets| offsets.len() == 2)
            .ok_or(Error::WeightsFormat)?;
        let start = offsets[0].as_u64().ok_or(Error::WeightsFormat)?;
        let end = offsets[1].as_u64().ok_or(Error::WeightsFormat)?;
        if end < start || end - start != expected || end > bytes - 8 - length {
            return Err(Error::WeightsFormat);
        }
        ranges.push((start, end));
    }
    if ranges.is_empty() {
        return Err(Error::WeightsFormat);
    }
    ranges.sort_unstable();
    let mut consumed = 0u64;
    for (start, end) in ranges {
        if start != consumed {
            return Err(Error::WeightsFormat);
        }
        consumed = end;
    }
    if consumed != bytes - 8 - length {
        return Err(Error::WeightsFormat);
    }
    Ok(())
}

impl Inspected {
    pub fn check_source(&self) -> Result<()> {
        if let Some(root) = &self.directory {
            for stamp in &self.stamps {
                let path = storage::child(root, &stamp.path)?;
                storage::plain(&path)?;
                let info = fs::metadata(path).map_err(|_| Error::SourceChanged)?;
                if !info.is_file()
                    || info.len() != stamp.bytes
                    || info.modified().ok() != stamp.modified
                {
                    return Err(Error::SourceChanged);
                }
            }
        }
        Ok(())
    }
}

fn gguf_hub_source(address: &str) -> Result<(HubSource, String)> {
    if address.len() > 2048
        || address.contains('\\')
        || address.to_ascii_lowercase().contains("%2e")
        || address.to_ascii_lowercase().contains("%2f")
        || address.to_ascii_lowercase().contains("%5c")
    {
        return Err(Error::SourceInvalid);
    }
    let parsed = url::Url::parse(address).map_err(|_| Error::SourceInvalid)?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("huggingface.co")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Error::SourceInvalid);
    }
    let parts: Vec<_> = parsed
        .path_segments()
        .ok_or(Error::SourceInvalid)?
        .collect();
    if parts.len() < 5
        || parts.len() > 20
        || !matches!(parts[2], "blob" | "resolve")
        || parts
            .iter()
            .enumerate()
            .any(|(index, part)| index != 2 && !segment(part))
    {
        return Err(Error::SourceInvalid);
    }
    let name = parts.last().ok_or(Error::SourceInvalid)?.to_string();
    if !name.to_ascii_lowercase().ends_with(".gguf") {
        return Err(Error::ModelFormatUnsupported);
    }
    Ok((
        HubSource {
            repository: format!("{}/{}", parts[0], parts[1]),
            revision: parts[3].to_owned(),
            subfolder: (parts.len() > 5).then(|| parts[4..parts.len() - 1].join("/")),
        },
        name,
    ))
}

async fn inspect_hub_gguf(
    address: &str,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Inspected> {
    let (source, name) = gguf_hub_source(address)?;
    let client = client()?;
    let bytes = fetch(
        &client,
        hub_url(&source, &source.revision, "api", None)?,
        MAX_METADATA_BYTES,
        cancelled,
        progress,
    )
    .await?;
    let metadata: HubMetadata =
        serde_json::from_slice(&bytes).map_err(|_| Error::SourceUnavailable)?;
    if !hex(&metadata.sha, 40) || metadata.siblings.len() > 2048 {
        return Err(Error::Manifest);
    }
    if hex(&source.revision, 40) && metadata.sha != source.revision {
        return Err(Error::SourceChanged);
    }
    let selected = match &source.subfolder {
        Some(folder) => format!("{folder}/{name}"),
        None => name.clone(),
    };
    let matching: Vec<_> = metadata
        .siblings
        .iter()
        .filter(|entry| entry.rfilename == selected)
        .collect();
    if matching.len() != 1 {
        return Err(Error::ModelManifestMissing);
    }
    let file = matching[0];
    let lfs = file.lfs.as_ref().ok_or(Error::SourceHashUnavailable)?;
    if !hex(&lfs.sha256, 64)
        || !(24..=MAX_DATA_BYTES).contains(&lfs.size)
        || file.size.is_some_and(|size| size != lfs.size)
    {
        return Err(Error::SourceHashUnavailable);
    }
    let label: String = format!("{} / {name}", source.repository)
        .chars()
        .take(160)
        .collect();
    let license = metadata
        .card
        .as_ref()
        .and_then(|card| card.get("license"))
        .and_then(Value::as_str)
        .filter(|license| text_label(license))
        .unwrap_or("License metadata unavailable")
        .to_owned();
    let artifact = Artifact {
        path: name.clone(),
        bytes: lfs.size,
        sha256: lfs.sha256.clone(),
        url: hub_url(&source, &metadata.sha, "resolve", Some(&name))?.to_string(),
        package: None,
        version: None,
        license: None,
    };
    Ok(Inspected {
        kind: "hugging_face_file",
        input_template: None,
        model: model(
            vec![artifact],
            Description {
                engine: EngineKind::LlamaCpp,
                native_format: Some("gguf-v3".to_owned()),
                revision: Some(metadata.sha.clone()),
                source: Some(hub_url(&source, &metadata.sha, "blob", Some(&name))?.to_string()),
                label,
                server_model_id: None,
                license,
            },
        )?,
        directory: None,
        stamps: Vec::new(),
    })
}

fn text_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_control)
}

fn inspect_local_gguf(
    path: &Path,
    cancelled: &Cancelled,
    progress: &Progress,
) -> Result<Inspected> {
    check(cancelled)?;
    storage::plain(path)?;
    let path = fs::canonicalize(path).map_err(|_| Error::Storage)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::SourceInvalid)?
        .to_owned();
    if !name.to_ascii_lowercase().ends_with(".gguf") {
        return Err(Error::ModelFormatUnsupported);
    }
    let before = fs::metadata(&path).map_err(|_| Error::Storage)?;
    let info = crate::gguf::inspect(&path, &std::sync::atomic::AtomicBool::new(false))?;
    let input_template = info
        .template
        .as_deref()
        .and_then(crate::gguf::builtin_template)
        .ok_or(Error::ModelFormatUnsupported)?;
    check(cancelled)?;
    let mut input = File::open(&path).map_err(|_| Error::Storage)?;
    let opened = input.metadata().map_err(|_| Error::Storage)?;
    if opened.len() != before.len() || opened.modified().ok() != before.modified().ok() {
        return Err(Error::SourceChanged);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut count = 0_u64;
    loop {
        check(cancelled)?;
        let read = input.read(&mut buffer).map_err(|_| Error::Storage)?;
        if read == 0 {
            break;
        }
        count = count.checked_add(read as u64).ok_or(Error::TooLarge)?;
        if count > before.len() {
            return Err(Error::SourceChanged);
        }
        digest.update(&buffer[..read]);
        progress(read as u64);
    }
    let after = input.metadata().map_err(|_| Error::Storage)?;
    if count != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(Error::SourceChanged);
    }
    let label: String = name
        .chars()
        .filter(|letter| !letter.is_control())
        .take(160)
        .collect();
    Ok(Inspected {
        kind: "local_file",
        input_template: Some(input_template.to_owned()),
        model: model(
            vec![Artifact {
                path: name.clone(),
                bytes: count,
                sha256: format!("{:x}", digest.finalize()),
                url: String::new(),
                package: None,
                version: None,
                license: None,
            }],
            Description {
                engine: EngineKind::LlamaCpp,
                native_format: Some("gguf-v3".to_owned()),
                revision: None,
                source: None,
                label,
                server_model_id: None,
                license: "License not inspected locally".to_owned(),
            },
        )?,
        directory: Some(path.parent().ok_or(Error::UnsafePath)?.to_owned()),
        stamps: vec![Stamp {
            path: name,
            bytes: before.len(),
            modified: before.modified().ok(),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_json_requires_the_pinned_git_blob_and_exact_size() {
        let bytes = br#"{"model_type":"qwen3_5"}"#;
        let file: HubFile = serde_json::from_value(serde_json::json!({"rfilename":"config.json","blobId":"05c6e9dfec32e8fee1aaaa5c0a94ad2c9736f878","size":bytes.len()})).unwrap();
        assert_eq!(verify_hub_json(&file, bytes), Ok(()));
        assert_eq!(
            verify_hub_json(&file, br#"{"model_type":"qwen3_6"}"#),
            Err(Error::Integrity)
        );
        assert_eq!(verify_hub_json(&file, b"{}"), Err(Error::Integrity));
    }

    #[test]
    fn hub_json_refuses_missing_hashes_and_unverified_lfs_bytes() {
        let missing: HubFile =
            serde_json::from_value(serde_json::json!({"rfilename":"config.json","size":2}))
                .unwrap();
        assert_eq!(
            verify_hub_json(&missing, b"{}"),
            Err(Error::SourceHashUnavailable)
        );
        let lfs: HubFile = serde_json::from_value(serde_json::json!({"rfilename":"config.json","size":2,"lfs":{"sha256":"a".repeat(64),"size":2}})).unwrap();
        assert_eq!(verify_hub_json(&lfs, b"{}"), Err(Error::Integrity));
    }

    #[test]
    fn hub_file_urls_use_the_resolved_commit_and_selected_subfolder() {
        let source = hub_source("acme/native", None, Some("checkpoint")).unwrap();
        let commit = "a".repeat(40);
        let address = hub_url(&source, &commit, "resolve", Some("model.safetensors")).unwrap();
        assert_eq!(
            address.as_str(),
            format!(
                "https://huggingface.co/acme/native/resolve/{commit}/checkpoint/model.safetensors"
            )
        );
        assert_eq!(
            hub_url(&source, "main", "api", None).unwrap().as_str(),
            "https://huggingface.co/api/models/acme/native/revision/main?blobs=true"
        );
    }
}
