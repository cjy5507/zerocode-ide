use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::catalog::{self, Artifact, Model};
use crate::{Error, Result, gguf, storage};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Runtime {
    pub archive: Artifact,
    pub executable: String,
    pub files: Vec<Artifact>,
}

impl Runtime {
    pub fn validate(&self) -> Result<()> {
        catalog::relative(&self.archive.path)?;
        catalog::relative(&self.executable)?;
        if self.archive.bytes == 0
            || self.archive.bytes > 64 * 1024 * 1024
            || self.archive.sha256.len() != 64
            || !self
                .archive
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !catalog::https(&self.archive.url)
            || self.files.is_empty()
            || self.files.len() > 128
        {
            return Err(Error::Manifest);
        }
        let mut total = 0_u64;
        let mut paths = std::collections::BTreeSet::new();
        for artifact in &self.files {
            let path = catalog::relative(&artifact.path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(Error::Manifest)?;
            total = total.checked_add(artifact.bytes).ok_or(Error::TooLarge)?;
            if !paths.insert(artifact.path.as_str())
                || artifact.bytes == 0
                || artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || artifact.url != self.archive.url
                || !(name == "llama-server" || name == "LICENSE" || name.ends_with(".dylib"))
            {
                return Err(Error::Manifest);
            }
        }
        if total > 256 * 1024 * 1024 || !paths.contains(self.executable.as_str()) {
            return Err(Error::Manifest);
        }
        Ok(())
    }

    pub fn file_path(&self, runtime: &Path, name: &str) -> Result<PathBuf> {
        storage::child(runtime, &format!("native/{name}"))
    }

    pub fn verify(&self, runtime: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
        self.validate()?;
        for artifact in &self.files {
            let path = self.file_path(runtime, &artifact.path)?;
            storage::verify(&path, artifact, cancel)?;
            if artifact.path == self.executable || artifact.path.ends_with(".dylib") {
                let mut header = [0; 8];
                File::open(&path)
                    .and_then(|mut file| file.read_exact(&mut header))
                    .map_err(|_| Error::RuntimeFailed)?;
                if header[..4] != [0xcf, 0xfa, 0xed, 0xfe] || header[4..] != [0x0c, 0, 0, 1] {
                    return Err(Error::Unsupported);
                }
            }
        }
        self.file_path(runtime, &self.executable)
    }

    pub fn initial(&self, runtime: &Path, model: Option<(&Path, &Model)>) -> Result<Value> {
        let executable = self.verify(runtime, &AtomicBool::new(false))?;
        let mut initial = json!({"nativeExecutable":executable});
        if let Some((directory, model)) = model {
            let (path, info, template) = model_info(directory, model)?;
            initial["nativeModelFile"] = json!(path);
            initial["inputTemplate"] = json!(template);
            initial["architecture"] = json!(info.architecture);
            initial["tensorCount"] = json!(info.tensors);
            initial["modelFileBytes"] = json!(info.file_bytes);
        }
        Ok(initial)
    }
}

pub(crate) fn model_info(
    directory: &Path,
    model: &Model,
) -> Result<(PathBuf, gguf::Info, &'static str)> {
    model_info_with_cancel(directory, model, &AtomicBool::new(false))
}

pub(crate) fn model_info_with_cancel(
    directory: &Path,
    model: &Model,
    cancel: &AtomicBool,
) -> Result<(PathBuf, gguf::Info, &'static str)> {
    if model.files.len() != 1 || !model.files[0].path.to_ascii_lowercase().ends_with(".gguf") {
        return Err(Error::ModelFormatUnsupported);
    }
    let path = storage::child(directory, &model.files[0].path)?;
    let info = gguf::inspect(&path, cancel)?;
    let template = info
        .template
        .as_deref()
        .and_then(gguf::builtin_template)
        .ok_or(Error::ModelFormatUnsupported)?;
    Ok((path, info, template))
}

#[cfg(test)]
mod tests;
