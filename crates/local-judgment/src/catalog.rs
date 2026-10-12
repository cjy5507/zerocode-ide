use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Laya,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub engine: EngineKind,
    pub protocol: judgment_provider::Protocol,
    pub revision: String,
    pub license: String,
    pub source: String,
    pub quality: String,
    pub files: Vec<Artifact>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    pub models: Vec<Model>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeProfile {
    pub id: String,
    pub python_version: String,
    pub engine: EngineKind,
    pub python: Artifact,
    pub executable: String,
    pub site_packages: String,
    pub wheels: Vec<Artifact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtimes {
    version: u32,
    profiles: Vec<RuntimeProfile>,
}

pub fn relative(path: &str) -> Result<PathBuf> {
    if path.is_empty()
        || path.len() > 512
        || path.contains(['\\', ':', '\0'])
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || part.ends_with(['.', ' '])
                || part.chars().any(char::is_control)
                || matches!(
                    part.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::UnsafePath);
    }
    Ok(PathBuf::from(path))
}

pub fn https(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

fn validate_files(files: &[Artifact]) -> Result<()> {
    if files.is_empty() || files.len() > 128 {
        return Err(Error::Manifest);
    }
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for file in files {
        relative(&file.path)?;
        if !names.insert(file.path.to_ascii_lowercase())
            || !https(&file.url)
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || file.bytes == 0
        {
            return Err(Error::Manifest);
        }
        total = total.checked_add(file.bytes).ok_or(Error::Manifest)?;
    }
    if total > 4 * 1024 * 1024 * 1024 {
        return Err(Error::TooLarge);
    }
    Ok(())
}

pub fn catalog() -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_str(include_str!("../manifests/models.json"))
        .map_err(|_| Error::Manifest)?;
    if catalog.version != 1 || catalog.models.len() > 32 {
        return Err(Error::Manifest);
    }
    let mut ids = BTreeSet::new();
    for model in &catalog.models {
        relative(&model.id)?;
        if model.id.contains('/') || !ids.insert(&model.id) || model.revision.len() != 40 {
            return Err(Error::Manifest);
        }
        validate_files(&model.files)?;
        if model
            .files
            .iter()
            .any(|file| !file.path.ends_with(".json") && !file.path.ends_with(".safetensors"))
        {
            return Err(Error::Manifest);
        }
    }
    Ok(catalog)
}

pub fn runtime() -> Result<Option<RuntimeProfile>> {
    let runtimes: Runtimes = serde_json::from_str(include_str!("../manifests/runtime.json"))
        .map_err(|_| Error::Manifest)?;
    if runtimes.version != 1 || runtimes.profiles.len() > 16 {
        return Err(Error::Manifest);
    }
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    for profile in runtimes.profiles {
        relative(&profile.id)?;
        relative(&profile.executable)?;
        relative(&profile.site_packages)?;
        validate_files(std::slice::from_ref(&profile.python))?;
        validate_files(&profile.wheels)?;
        if profile.id == platform {
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

impl Model {
    pub fn download_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    pub fn fingerprint(&self) -> Result<String> {
        digest(self)
    }
}

impl RuntimeProfile {
    pub fn download_bytes(&self) -> u64 {
        self.python.bytes + self.wheels.iter().map(|file| file.bytes).sum::<u64>()
    }

    pub fn fingerprint(&self) -> Result<String> {
        digest(self)
    }
}

fn digest(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Manifest)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
