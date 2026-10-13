use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub const MAX_CATALOG_BYTES: usize = 512 * 1024;
pub const MAX_MODELS: usize = 32;

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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Laya,
    TransformersSystemOne,
    SystemOneServer,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_format: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_files: Vec<Artifact>,
    #[serde(default = "validated", skip_serializing_if = "is_validated")]
    pub validated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rss_bytes: Option<u64>,
}

fn validated() -> bool {
    true
}

fn is_validated(value: &bool) -> bool {
    *value
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeExtension {
    id: String,
    engine: EngineKind,
    base_profiles: Vec<String>,
    remove_packages: Vec<String>,
    wheels: Vec<Artifact>,
    platform_wheels: std::collections::BTreeMap<String, Vec<Artifact>>,
    runtime_files: Vec<Artifact>,
    validated_platforms: Vec<String>,
    max_rss_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeExtensions {
    version: u32,
    extensions: Vec<RuntimeExtension>,
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

pub fn local_source(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| {
        url.scheme() == "local"
            && url.host_str().is_some_and(|host| hex(host, 64))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && (url.path().is_empty()
                || url.path() == "/"
                || relative(url.path().trim_start_matches('/')).is_ok())
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
            || (!https(&file.url) && !local_source(&file.url))
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || file.bytes == 0
        {
            return Err(Error::Manifest);
        }
        total = total.checked_add(file.bytes).ok_or(Error::Manifest)?;
    }
    for name in &names {
        let mut ancestor = name.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if names.contains(parent) {
                return Err(Error::Manifest);
            }
            ancestor = parent;
        }
    }
    if total > 4 * 1024 * 1024 * 1024 {
        return Err(Error::TooLarge);
    }
    Ok(())
}

pub fn catalog() -> Result<Catalog> {
    let catalog = Catalog::parse(include_str!("../manifests/models.json"))?;
    catalog.merge(&Catalog::parse(include_str!(
        "../manifests/native-models.json"
    ))?)
}

impl Catalog {
    pub fn parse(document: &str) -> Result<Self> {
        if document.len() > MAX_CATALOG_BYTES {
            return Err(Error::TooLarge);
        }
        let catalog: Self = serde_json::from_str(document).map_err(|_| Error::Manifest)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.models.is_empty() || self.models.len() > MAX_MODELS {
            return Err(Error::Manifest);
        }
        let mut ids = BTreeSet::new();
        for model in &self.models {
            model.validate()?;
            if !ids.insert(model.id.to_ascii_lowercase()) {
                return Err(Error::CatalogConflict);
            }
        }
        Ok(())
    }

    pub fn merge(&self, addition: &Self) -> Result<Self> {
        addition.validate()?;
        if self.models.len().saturating_add(addition.models.len()) > MAX_MODELS {
            return Err(Error::TooLarge);
        }
        let mut merged = self.clone();
        for model in &addition.models {
            if merged
                .models
                .iter()
                .any(|existing| existing.id.eq_ignore_ascii_case(&model.id))
            {
                return Err(Error::CatalogConflict);
            }
            merged.models.push(model.clone());
        }
        Ok(merged)
    }

    pub fn fingerprint(&self) -> Result<String> {
        let mut catalog = self.clone();
        catalog.models.sort_by(|left, right| left.id.cmp(&right.id));
        digest(&catalog)
    }
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

pub fn runtime_profiles() -> Result<Vec<RuntimeProfile>> {
    let Some(base) = runtime()? else {
        return Ok(Vec::new());
    };
    let extensions: RuntimeExtensions =
        serde_json::from_str(include_str!("../manifests/native-runtime.json"))
            .map_err(|_| Error::Manifest)?;
    if extensions.version != 1 || extensions.extensions.len() > 8 {
        return Err(Error::Manifest);
    }
    let mut profiles = vec![base.clone()];
    for extension in extensions.extensions {
        if !extension.base_profiles.contains(&base.id) {
            continue;
        }
        relative(&extension.id)?;
        if extension.max_rss_bytes == 0
            || extension.max_rss_bytes > 8 * 1024 * 1024 * 1024
            || profiles
                .iter()
                .any(|profile| profile.engine == extension.engine)
        {
            return Err(Error::Manifest);
        }
        let mut profile = base.clone();
        profile.id = format!("{}-{}", base.id, extension.id);
        profile.engine = extension.engine;
        profile.wheels.retain(|wheel| {
            !wheel
                .package
                .as_ref()
                .is_some_and(|package| extension.remove_packages.contains(package))
        });
        profile.wheels.extend(extension.wheels);
        profile.wheels.extend(
            extension
                .platform_wheels
                .get(&base.id)
                .ok_or(Error::Manifest)?
                .iter()
                .cloned(),
        );
        profile.runtime_files = extension.runtime_files;
        profile.validated = extension.validated_platforms.contains(&base.id);
        profile.max_rss_bytes = Some(extension.max_rss_bytes);
        validate_files(&profile.wheels)?;
        validate_files(&profile.runtime_files)?;
        profiles.push(profile);
    }
    Ok(profiles)
}

impl Model {
    fn validate(&self) -> Result<()> {
        relative(&self.id)?;
        if self.id.len() > 96
            || !self.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
            || !hex(&self.revision, 40)
            || !text(&self.label, 160)
            || !text(&self.license, 160)
            || !text(&self.quality, 80)
            || self.source.len() > 2048
            || (!https(&self.source) && !local_source(&self.source))
            || self.protocol != judgment_provider::Protocol::SystemOne
            || self
                .server_model_id
                .as_ref()
                .is_some_and(|id| !text(id, 256))
            || self
                .weights_bytes
                .is_some_and(|bytes| bytes == 0 || bytes > 4 * 1024 * 1024 * 1024)
            || self
                .native_format
                .as_ref()
                .is_some_and(|format| !text(format, 80))
        {
            return Err(Error::Manifest);
        }
        if self.files.is_empty() {
            return Ok(());
        }
        validate_files(&self.files)?;
        let weights_bytes: u64 = self
            .files
            .iter()
            .filter(|file| file.path.ends_with(".safetensors"))
            .map(|file| file.bytes)
            .sum();
        if weights_bytes != 0
            && self
                .weights_bytes
                .is_some_and(|declared| declared != weights_bytes)
        {
            return Err(Error::Manifest);
        }
        for file in &self.files {
            if (!file.path.ends_with(".json") && !file.path.ends_with(".safetensors"))
                || file.path.eq_ignore_ascii_case("receipt.json")
                || file.path.eq_ignore_ascii_case("ownership.json")
                || file.url.len() > 2048
                || file.package.is_some()
                || file.version.is_some()
                || file.license.is_some()
                || !hex(&file.sha256, 64)
            {
                return Err(Error::Manifest);
            }
        }
        Ok(())
    }

    pub fn weights_bytes(&self) -> u64 {
        self.weights_bytes.unwrap_or_else(|| {
            self.files
                .iter()
                .filter(|file| file.path.ends_with(".safetensors"))
                .map(|file| file.bytes)
                .sum()
        })
    }

    pub fn download_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    pub fn fingerprint(&self) -> Result<String> {
        digest(self)
    }
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

impl RuntimeProfile {
    pub fn download_bytes(&self) -> u64 {
        self.python.bytes
            + self
                .wheels
                .iter()
                .chain(&self.runtime_files)
                .map(|file| file.bytes)
                .sum::<u64>()
    }

    pub fn fingerprint(&self) -> Result<String> {
        let mut runtime = self.clone();
        runtime.validated = true;
        digest(&runtime)
    }
}

fn digest(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Manifest)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
