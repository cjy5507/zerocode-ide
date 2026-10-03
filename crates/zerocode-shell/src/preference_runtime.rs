use std::io::{self, Read};
use std::path::{Path, PathBuf};

use zerocode_core::user_preferences::{
    DIRECTORY_NAME, FILE_NAME, FeedbackOrigin, MAX_BYTES, PreferenceBook, PreferenceScope,
    SavedPreference,
};

use crate::durable_file;

pub(crate) struct PreferenceStore {
    root: PathBuf,
}

impl PreferenceStore {
    pub(crate) fn at(config_home: &Path) -> Self {
        let root = config_home
            .canonicalize()
            .unwrap_or_else(|_| config_home.to_path_buf());
        Self {
            root: root.join(DIRECTORY_NAME),
        }
    }

    pub(crate) fn of_this_machine() -> Result<Self, String> {
        let settings =
            crate::api_routers::zo_settings_path().ok_or("zo configuration home is unavailable")?;
        let home = settings
            .parent()
            .ok_or("zo configuration home is invalid")?;
        Ok(Self::at(home))
    }

    fn read(&self) -> Result<PreferenceBook, String> {
        durable_file::require_plain_directory_if_present(&self.root)
            .map_err(|_| "the preference directory is not a plain directory")?;
        let file = match durable_file::open_plain_file(&self.root.join(FILE_NAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PreferenceBook::default());
            }
            Err(_) => return Err("saved preferences could not be read safely".into()),
        };
        let cap = u64::try_from(MAX_BYTES).map_err(|_| "invalid preference size limit")?;
        if file
            .metadata()
            .map_err(|_| "saved preference metadata is unavailable")?
            .len()
            > cap
        {
            return Err("saved preference state exceeds its size limit".into());
        }
        let mut bytes = Vec::new();
        file.take(cap + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "saved preferences could not be read")?;
        PreferenceBook::parse(&bytes).map_err(|error| error.message().to_owned())
    }

    pub(crate) fn list(&self) -> Result<Vec<SavedPreference>, String> {
        Ok(self.read()?.entries().to_vec())
    }

    fn modify<T>(
        &self,
        apply: impl FnOnce(&mut PreferenceBook) -> Result<T, String>,
    ) -> Result<T, String> {
        durable_file::ensure_private_directory_durable(&self.root)
            .map_err(|_| "the private preference directory is unavailable")?;
        let lock = durable_file::private_lock_file(&self.root.join(".user-preferences.lock"))
            .map_err(|_| "the preference store could not be locked safely")?;
        lock.lock()
            .map_err(|_| "the preference store could not be locked")?;
        let mut book = self.read()?;
        let result = apply(&mut book)?;
        let bytes = book.encode().map_err(|error| error.message().to_owned())?;
        let written = durable_file::replace_bytes(&self.root.join(FILE_NAME), &bytes)
            .map_err(|_| "the preference state could not be saved")?;
        if !written.platform_durable() {
            return Err(
                "preference state changed, but directory synchronization failed; retry saving"
                    .into(),
            );
        }
        Ok(result)
    }

    pub(crate) fn save(
        &self,
        text: &str,
        scope: PreferenceScope,
        origin: FeedbackOrigin,
        now_ms: i64,
    ) -> Result<String, String> {
        self.modify(|book| {
            book.save(text, scope, origin, now_ms)
                .map_err(|error| error.message().to_owned())
        })
    }

    pub(crate) fn revoke(&self, id: &str) -> Result<bool, String> {
        self.modify(|book| book.revoke(id).map_err(|error| error.message().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> FeedbackOrigin {
        FeedbackOrigin {
            artifact_id: "page-example".into(),
            version: 2,
            sha256: Some("a".repeat(64)),
            feedback_key: "b".repeat(64),
            followed_link: false,
        }
    }

    #[test]
    fn preferences_persist_across_reopen_and_revocation_keeps_an_explicit_empty_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = PreferenceStore::at(directory.path());
        let id = store
            .save(
                "Prefer concise headings",
                PreferenceScope::Personal,
                origin(),
                10,
            )
            .unwrap();
        let reopened = PreferenceStore::at(directory.path());
        assert_eq!(reopened.list().unwrap()[0].id, id);
        assert!(reopened.revoke(&id).unwrap());
        assert!(store.list().unwrap().is_empty());
        assert!(store.root.join(FILE_NAME).is_file());
    }

    #[test]
    fn damaged_state_is_neither_empty_nor_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let store = PreferenceStore::at(directory.path());
        std::fs::create_dir_all(&store.root).unwrap();
        let path = store.root.join(FILE_NAME);
        std::fs::write(&path, b"damaged").unwrap();
        assert!(store.list().is_err());
        assert!(
            store
                .save(
                    "Use concise headings",
                    PreferenceScope::Personal,
                    origin(),
                    10
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged");
    }

    #[test]
    fn independent_writers_keep_each_others_preferences() {
        let directory = tempfile::tempdir().unwrap();
        std::thread::scope(|threads| {
            let writers: Vec<_> = (0..8)
                .map(|index| {
                    let home = directory.path();
                    threads.spawn(move || {
                        PreferenceStore::at(home).save(
                            &format!("Preference number {index}"),
                            PreferenceScope::Personal,
                            origin(),
                            10,
                        )
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap().unwrap();
            }
        });
        let entries = PreferenceStore::at(directory.path()).list().unwrap();
        assert_eq!(entries.len(), 8);
        for index in 0..8 {
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.text == format!("Preference number {index}"))
            );
        }
    }

    #[test]
    fn oversized_state_is_refused_before_a_writer_can_replace_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = PreferenceStore::at(directory.path());
        std::fs::create_dir_all(&store.root).unwrap();
        let state = vec![b' '; MAX_BYTES + 1];
        let file = store.root.join(FILE_NAME);
        std::fs::write(&file, &state).unwrap();
        assert!(store.list().is_err());
        assert!(
            store
                .save(
                    "Keep headings concise",
                    PreferenceScope::Personal,
                    origin(),
                    10
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&file).unwrap(), state);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_preference_directory_is_neither_read_nor_written() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = PreferenceStore::at(directory.path());
        std::os::unix::fs::symlink(outside.path(), &store.root).unwrap();
        assert!(store.list().is_err());
        assert!(
            store
                .save(
                    "Keep headings concise",
                    PreferenceScope::Personal,
                    origin(),
                    10
                )
                .is_err()
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_preference_symlink_is_not_followed_or_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let store = PreferenceStore::at(directory.path());
        std::fs::create_dir_all(&store.root).unwrap();
        let outside = directory.path().join("outside.json");
        std::fs::write(&outside, b"private outside contents").unwrap();
        std::os::unix::fs::symlink(&outside, store.root.join(FILE_NAME)).unwrap();
        assert!(store.list().is_err());
        assert!(
            store
                .save(
                    "Use concise headings",
                    PreferenceScope::Personal,
                    origin(),
                    10
                )
                .is_err()
        );
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"private outside contents"
        );
    }
}
