use std::path::Path;

use judgment_provider::ProviderKind;
use serde::{Deserialize, Serialize};

use crate::catalog::Model;
use crate::selection::Selected;
use crate::{Error, Result, storage};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunIntent {
    version: u32,
    pub model_id: String,
    pub revision: String,
    pub manifest_fingerprint: String,
    pub provider_identity: String,
    pub port: u16,
    at_ms: u64,
}

impl RunIntent {
    pub fn read(path: &Path) -> Option<Self> {
        let bytes = storage::bounded_read(path, MAX_BYTES).ok()?;
        let intent: Self = serde_json::from_slice(&bytes).ok()?;
        (intent.version == VERSION && intent.port != 0).then_some(intent)
    }

    pub fn new(model: &Model, provider_identity: &str, port: u16, at_ms: u64) -> Result<Self> {
        Ok(Self {
            version: VERSION,
            model_id: model.id.clone(),
            revision: model.revision.clone(),
            manifest_fingerprint: model.fingerprint()?,
            provider_identity: provider_identity.to_owned(),
            port,
            at_ms,
        })
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Storage)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        storage::replace(path, &bytes)
    }

    pub fn matches(&self, selected: &Selected) -> bool {
        selected.provider.kind == ProviderKind::Local
            && selected.provider.identity() == self.provider_identity
            && selected.binding.as_ref().is_some_and(|binding| {
                binding.matches(&selected.provider)
                    && binding.model_id == self.model_id
                    && binding.revision == self.revision
                    && binding.provider_identity == self.provider_identity
            })
    }
}
