use std::fs::File;
use std::io::Read;
use std::path::Path;

use judgment_provider::{Provider, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

pub const MANAGED_MODEL_KEY: &str = "managedJudgmentModel";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub model_id: String,
    pub revision: String,
    pub provider_identity: String,
}

impl Binding {
    pub fn matches(&self, provider: &Provider) -> bool {
        provider.kind == ProviderKind::Local
            && self.model_id == provider.model
            && self.provider_identity == provider.identity()
            && self.revision.len() == 40
            && self.revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

pub struct Selected {
    pub provider: Provider,
    pub binding: Option<Binding>,
}

pub fn read(path: &Path) -> Result<Selected> {
    let root = match File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(judgment_provider::MAX_RESPONSE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::Storage)?;
            if bytes.len() > judgment_provider::MAX_RESPONSE_BYTES {
                return Err(Error::TooLarge);
            }
            let root: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
            if !root.is_object() {
                return Err(Error::Storage);
            }
            root
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(_) => return Err(Error::Storage),
    };
    let provider = Provider::from_root(&root).map_err(|_| Error::SettingsChanged)?;
    let binding = root
        .get("smart")
        .and_then(|smart| smart.get(MANAGED_MODEL_KEY))
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value::<Binding>(value.clone()).map_err(|_| Error::Storage))
        .transpose()?
        .filter(|binding| binding.matches(&provider));
    Ok(Selected { provider, binding })
}
