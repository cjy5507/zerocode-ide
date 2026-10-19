use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

#[derive(Debug)]
pub(crate) struct Info {
    pub architecture: String,
    pub template: Option<String>,
    pub tensors: u64,
    pub file_bytes: u64,
}

pub(crate) fn inspect(_path: &Path, cancel: &AtomicBool) -> Result<Info> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    Err(Error::WeightsFormat)
}
