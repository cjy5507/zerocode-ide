use std::path::{Path, PathBuf};

use zerocode_core::scm_observer::{Book, Effects, Failure, HealthBook, HealthChange, Operation};

use crate::gh::GhError;

/// The health a tick judges by, and what it heard. The tick asks GitHub with
/// the observer free, so what it heard is written into the book's health as
/// that stands when the tick closes, not over the copy the tick started from:
/// a retry a person asked for meanwhile stays (t-47457).
pub(super) struct TickHealth {
    /// The book's health when the tick was planned, with what the tick has
    /// heard since: what is due is judged here.
    book: HealthBook,
    heard: Vec<Heard>,
    now_ms: i64,
}

/// One thing a tick heard about an operation's health, in the order heard.
enum Heard {
    Observed {
        root: PathBuf,
        operation: Operation,
        context: String,
        result: Result<(), Failure>,
    },
    Forgotten {
        root: PathBuf,
        operation: Operation,
    },
}

impl TickHealth {
    pub fn new(book: &Book, now_ms: i64) -> Self {
        Self {
            book: book.health.clone(),
            heard: Vec::new(),
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
        self.book.record(
            &root.to_string_lossy(),
            operation,
            context,
            observed,
            self.now_ms,
        );
        self.heard.push(Heard::Observed {
            root: root.to_path_buf(),
            operation,
            context: context.to_owned(),
            result: observed,
        });
    }

    pub fn forget(&mut self, root: &Path, operation: Operation) {
        self.book.forget(&root.to_string_lossy(), operation);
        self.heard.push(Heard::Forgotten {
            root: root.to_path_buf(),
            operation,
        });
    }

    /// Writes what the tick heard, in order, into `book`'s health as it
    /// stands now, and logs each operation that failed or recovered there.
    pub fn commit(
        self,
        book: &mut Book,
        effects: &mut impl Effects,
        log_root: &Path,
    ) -> Result<(), String> {
        let mut health = book.health.clone();
        let mut changes = Vec::new();
        for heard in self.heard {
            match heard {
                Heard::Observed {
                    root,
                    operation,
                    context,
                    result,
                } => {
                    if let Some(change) = health.record(
                        &root.to_string_lossy(),
                        operation,
                        &context,
                        result,
                        self.now_ms,
                    ) {
                        changes.push((root, operation, change));
                    }
                }
                Heard::Forgotten { root, operation } => {
                    health.forget(&root.to_string_lossy(), operation);
                }
            }
        }
        book.update_health(health, effects)?;
        for (root, operation, change) in changes {
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
