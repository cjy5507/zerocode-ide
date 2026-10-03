use std::path::{Path, PathBuf};

use zerocode_core::scm_observer::{Book, Effects, HealthBook, HealthChange, Operation};

use crate::gh::GhError;

pub(super) struct TickHealth {
    book: HealthBook,
    changes: Vec<(PathBuf, Operation, HealthChange)>,
    now_ms: i64,
}

impl TickHealth {
    pub fn new(book: &Book, now_ms: i64) -> Self {
        Self {
            book: book.health.clone(),
            changes: Vec::new(),
            now_ms,
        }
    }

    pub fn due(&self, root: &Path, operation: Operation, context: &str) -> bool {
        self.book
            .due(&root.to_string_lossy(), operation, context, self.now_ms)
    }

    pub fn observe<T>(
        &mut self,
        root: &Path,
        operation: Operation,
        context: &str,
        result: &Result<T, GhError>,
    ) {
        let observed = match result {
            Ok(_) => Ok(()),
            Err(error) => match error.fetch_failure() {
                Some(failure) => Err(failure),
                None => return,
            },
        };
        if let Some(change) = self.book.record(
            &root.to_string_lossy(),
            operation,
            context,
            observed,
            self.now_ms,
        ) {
            self.changes.push((root.to_path_buf(), operation, change));
        }
    }

    pub fn forget(&mut self, root: &Path, operation: Operation) {
        self.book.forget(&root.to_string_lossy(), operation);
    }

    pub fn commit(
        self,
        book: &mut Book,
        effects: &mut impl Effects,
        log_root: &Path,
    ) -> Result<(), String> {
        book.update_health(self.book, effects)?;
        for (root, operation, change) in self.changes {
            let status = match change {
                HealthChange::Failed(kind) => kind.token(),
                HealthChange::Recovered => "recovered",
            };
            crate::note_window_event(
                log_root,
                &format!(
                    "scm-observer: {} fetch {status} · {}",
                    operation.token(),
                    root.display()
                ),
            );
        }
        Ok(())
    }
}
