//! Explicit retention management: persist a complete private export before
//! retiring any active evidence. Only one case and its labels are loaded at once.

use super::*;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveReport {
    pub archived: usize,
    pub retired: usize,
    pub invalid: usize,
    pub changed: usize,
}

fn evidence(path: &Path) -> io::Result<(Case, Vec<Outcome>, String)> {
    let case = read_case(path)?;
    let (outcomes, invalid) = read_outcomes(path, &case);
    if invalid != 0 {
        return Err(io::Error::other("invalid outcomes cannot be retired"));
    }
    let fingerprint = super::super::digest(&(&case, &outcomes)).map_err(io::Error::other)?;
    Ok((case, outcomes, fingerprint))
}

impl Store {
    /// Archive selected evidence to a new private JSON export, then free its
    /// active slots. Unreviewed evidence stays unless explicitly included.
    /// Existing archives are never overwritten; failed exports retire nothing.
    ///
    /// # Errors
    /// Busy/corrupt storage, an existing or unsafe destination, or an I/O error.
    /// If retiring files fails, the completed archive still holds their evidence.
    pub fn archive(
        &self,
        destination: &Path,
        seat: Option<&str>,
        rubric: Option<u32>,
        include_unreviewed: bool,
    ) -> io::Result<ArchiveReport> {
        let _lock = self.lock()?;
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if parent
            .canonicalize()?
            .starts_with(self.root.canonicalize()?)
        {
            return Err(io::Error::other(
                "archive must be outside the active review store",
            ));
        }
        let mut result = ArchiveReport::default();
        let mut selected = Vec::new();
        for path in self.paths()? {
            let Ok((case, outcomes, fingerprint)) = evidence(&path) else {
                result.invalid += 1;
                continue;
            };
            if seat.is_some_and(|seat| case.seat != seat)
                || rubric.is_some_and(|version| case.rubric_version != version)
            {
                continue;
            }
            let reviewed = case.request["questions"]
                .as_object()
                .is_some_and(|questions| {
                    questions.keys().all(|question| {
                        outcomes.iter().any(|outcome| &outcome.question == question)
                    })
                });
            if reviewed || include_unreviewed {
                selected.push((path, fingerprint));
            }
        }
        let mut output = private_options().create_new(true).open(destination)?;
        let written = (|| -> io::Result<()> {
            output.write_all(b"{\"schemaVersion\":1,\"invalid\":0,\"cases\":[")?;
            for (index, (path, expected)) in selected.iter().enumerate() {
                let (case, _, actual) = evidence(path)?;
                if &actual != expected {
                    return Err(io::Error::other("evidence changed during archive"));
                }
                if index != 0 {
                    output.write_all(b",")?;
                }
                serde_json::to_writer(&mut output, &case).map_err(io::Error::other)?;
            }
            output.write_all(b"],\"outcomes\":[")?;
            let mut first = true;
            for (path, expected) in &selected {
                let (_, outcomes, actual) = evidence(path)?;
                if &actual != expected {
                    return Err(io::Error::other("reviews changed during archive"));
                }
                for outcome in outcomes {
                    if !first {
                        output.write_all(b",")?;
                    }
                    first = false;
                    serde_json::to_writer(&mut output, &outcome).map_err(io::Error::other)?;
                }
            }
            output.write_all(b"]}\n")?;
            output.sync_all()?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        drop(output);
        if let Err(error) = written {
            let _ = fs::remove_file(destination);
            return Err(error);
        }
        result.archived = selected.len();
        // The archive is durable before the first removal. Recheck content
        // again so an out-of-band editor cannot lose a newer review.
        for (path, expected) in selected {
            if evidence(&path).is_ok_and(|(_, _, actual)| actual == expected) {
                fs::remove_file(&path)?;
                result.retired += 1;
                match fs::remove_file(path.with_extension("outcomes")) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            } else {
                result.changed += 1;
            }
        }
        Ok(result)
    }
}
