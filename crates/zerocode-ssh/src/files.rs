//! File operations shared by the two panes and the transfer worker. File bytes
//! stay on the native side; recursion never follows symbolic links.
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use russh_sftp::{
    client::{SftpSession, error::Error as SftpError},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt, SeekFrom,
};

const BUFFER_BYTES: usize = 64 * 1024;
const MAX_TREE_ENTRIES: usize = 200_000;
pub const EDIT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Sftp(#[from] SftpError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("transfer cancelled")]
    Cancelled,
    #[error("destination already exists: {0}")]
    Exists(String),
}
pub type Result<T> = std::result::Result<T, FileError>;

#[derive(Clone)]
pub enum FileSystem {
    Local,
    Remote(Arc<SftpSession>),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
    pub modified: Option<u32>,
    pub permissions: Option<u32>,
}

impl Entry {
    fn new(path: String, attrs: FileAttributes) -> Self {
        Self {
            name: path.rsplit('/').next().unwrap_or(&path).to_string(),
            path,
            directory: attrs.is_dir(),
            symlink: attrs.is_symlink(),
            size: attrs.size.unwrap_or(0),
            modified: attrs.mtime,
            permissions: attrs.permissions.map(|mode| mode & 0o7777),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
}

fn valid(path: &str) -> Result<()> {
    if path.is_empty() || path.contains('\0') {
        return Err(FileError::Invalid("invalid file path"));
    }
    Ok(())
}

pub fn child(parent: &str, name: &str) -> Result<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(FileError::Invalid("invalid file name"));
    }
    Ok(format!("{}/{name}", parent.trim_end_matches('/')))
}

fn missing(error: &FileError) -> bool {
    match error {
        FileError::Io(error) => error.kind() == std::io::ErrorKind::NotFound,
        FileError::Sftp(SftpError::Status(status)) => status.status_code == StatusCode::NoSuchFile,
        _ => false,
    }
}

trait FileStream: AsyncRead + AsyncWrite + AsyncSeek + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + AsyncSeek + Unpin + Send> FileStream for T {}
type Stream = Box<dyn FileStream>;

impl FileSystem {
    pub async fn canonicalize(&self, path: &str) -> Result<String> {
        valid(path)?;
        let path = match self {
            Self::Local => tokio::fs::canonicalize(path)
                .await?
                .to_string_lossy()
                .into_owned(),
            Self::Remote(sftp) => sftp.canonicalize(path).await?,
        };
        valid(&path)?;
        if matches!(self, Self::Remote(_)) && !path.starts_with('/') {
            return Err(FileError::Invalid("server returned a relative path"));
        }
        Ok(path)
    }

    pub async fn metadata(&self, path: &str) -> Result<Entry> {
        valid(path)?;
        let attrs = match self {
            Self::Local => {
                let metadata = tokio::fs::symlink_metadata(path).await?;
                let mut attrs = FileAttributes::from(&metadata);
                if metadata.is_symlink() {
                    attrs.permissions = Some((attrs.permissions.unwrap_or(0) & 0o7777) | 0o120000);
                }
                attrs
            }
            Self::Remote(sftp) => sftp.symlink_metadata(path).await?,
        };
        Ok(Entry::new(path.to_string(), attrs))
    }

    pub async fn exists(&self, path: &str) -> Result<Option<Entry>> {
        match self.metadata(path).await {
            Ok(entry) => Ok(Some(entry)),
            Err(error) if missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn list(&self, path: &str) -> Result<Directory> {
        let path = self.canonicalize(path).await?;
        let mut entries = Vec::new();
        match self {
            Self::Local => {
                let mut directory = tokio::fs::read_dir(&path).await?;
                while let Some(entry) = directory.next_entry().await? {
                    entries.push(self.metadata(&entry.path().to_string_lossy()).await?);
                    if entries.len() > MAX_TREE_ENTRIES {
                        return Err(FileError::Invalid("directory entry limit exceeded"));
                    }
                }
            }
            Self::Remote(sftp) => {
                for entry in sftp.read_dir(&path).await? {
                    let name = entry.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    entries.push(Entry::new(child(&path, &name)?, entry.metadata()));
                    if entries.len() > MAX_TREE_ENTRIES {
                        return Err(FileError::Invalid("directory entry limit exceeded"));
                    }
                }
            }
        }
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let parent = match self {
            Self::Local => Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned()),
            Self::Remote(_) if path == "/" => None,
            Self::Remote(_) => Some(
                path.rsplit_once('/')
                    .map(|(p, _)| if p.is_empty() { "/" } else { p })
                    .unwrap_or("/")
                    .to_string(),
            ),
        };
        Ok(Directory {
            path,
            parent,
            entries,
        })
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        valid(path)?;
        match self {
            Self::Local => tokio::fs::create_dir(path).await?,
            Self::Remote(sftp) => sftp.create_dir(path).await?,
        }
        Ok(())
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.check_destination(from, to).await?;
        if self.exists(to).await?.is_some() {
            return Err(FileError::Exists(to.to_string()));
        }
        match self {
            Self::Local => tokio::fs::rename(from, to).await?,
            Self::Remote(sftp) => sftp.rename(from, to).await?,
        }
        Ok(())
    }

    async fn check_destination(&self, from: &str, to: &str) -> Result<()> {
        valid(to)?;
        let source = self.canonicalize(from).await?;
        let parent = Path::new(to)
            .parent()
            .and_then(Path::to_str)
            .ok_or(FileError::Invalid("destination needs a parent directory"))?;
        let parent = self.canonicalize(parent).await?;
        let name = Path::new(to)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(FileError::Invalid("invalid destination name"))?;
        let destination = child(&parent, name)?;
        if destination == source
            || destination.starts_with(&format!("{}/", source.trim_end_matches('/')))
        {
            return Err(FileError::Invalid("cannot copy or move a path into itself"));
        }
        Ok(())
    }

    pub async fn chmod(
        &self,
        path: &str,
        mode: u32,
        recursive: bool,
        control: &TransferControl,
    ) -> Result<()> {
        if mode > 0o7777 {
            return Err(FileError::Invalid("invalid octal permissions"));
        }
        let entries = if recursive {
            self.walk(path, control).await?
        } else {
            vec![self.metadata(path).await?]
        };
        // Children must remain reachable until their own permissions change.
        for entry in entries.into_iter().rev() {
            control.check().await?;
            if entry.symlink {
                continue;
            }
            match self {
                Self::Local => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        tokio::fs::set_permissions(
                            &entry.path,
                            std::fs::Permissions::from_mode(mode),
                        )
                        .await?;
                    }
                    #[cfg(not(unix))]
                    {
                        return Err(FileError::Invalid(
                            "POSIX permissions are unavailable on this local filesystem",
                        ));
                    }
                }
                Self::Remote(sftp) => {
                    sftp.set_metadata(
                        &entry.path,
                        FileAttributes {
                            permissions: Some(mode),
                            ..FileAttributes::empty()
                        },
                    )
                    .await?
                }
            }
        }
        Ok(())
    }

    pub async fn walk(&self, path: &str, control: &TransferControl) -> Result<Vec<Entry>> {
        let mut pending = vec![self.metadata(path).await?];
        let mut entries = Vec::new();
        while let Some(entry) = pending.pop() {
            control.check().await?;
            if entry.directory && !entry.symlink {
                pending.extend(self.list(&entry.path).await?.entries);
            }
            entries.push(entry);
            if entries.len() + pending.len() > MAX_TREE_ENTRIES {
                return Err(FileError::Invalid("recursive entry limit exceeded"));
            }
        }
        Ok(entries)
    }

    pub async fn remove(&self, path: &str, control: &TransferControl) -> Result<()> {
        let spelling = path.trim_end_matches('/');
        if spelling.is_empty()
            || Path::new(spelling).parent().is_none()
            || spelling.ends_with("/.")
            || spelling.ends_with("/..")
        {
            return Err(FileError::Invalid("cannot remove a filesystem root"));
        }
        if !self.metadata(path).await?.symlink
            && Path::new(&self.canonicalize(path).await?)
                .parent()
                .is_none()
        {
            return Err(FileError::Invalid("cannot remove a filesystem root"));
        }
        let entries = self.walk(path, control).await?;
        for entry in entries.into_iter().rev() {
            control.check().await?;
            match (self, entry.directory && !entry.symlink) {
                (Self::Local, true) => tokio::fs::remove_dir(&entry.path).await?,
                (Self::Local, false) => tokio::fs::remove_file(&entry.path).await?,
                (Self::Remote(sftp), true) => sftp.remove_dir(&entry.path).await?,
                (Self::Remote(sftp), false) => sftp.remove_file(&entry.path).await?,
            }
        }
        Ok(())
    }

    async fn open(
        &self,
        path: &str,
        write: bool,
        truncate: bool,
        exclusive: bool,
    ) -> Result<Stream> {
        valid(path)?;
        Ok(match self {
            Self::Local => {
                let mut options = tokio::fs::OpenOptions::new();
                options
                    .read(!write)
                    .write(write)
                    .create(write)
                    .truncate(truncate)
                    .create_new(exclusive);
                Box::new(options.open(path).await?)
            }
            Self::Remote(sftp) => {
                let mut flags = if write {
                    OpenFlags::WRITE
                } else {
                    OpenFlags::READ
                };
                if write {
                    flags |= OpenFlags::WRITE | OpenFlags::CREATE;
                }
                if truncate {
                    flags |= OpenFlags::TRUNCATE;
                }
                if exclusive {
                    flags |= OpenFlags::EXCLUDE;
                }
                Box::new(sftp.open_with_flags(path, flags).await?)
            }
        })
    }

    pub async fn read_text(&self, path: &str) -> Result<String> {
        let entry = self.metadata(path).await?;
        if entry.size > EDIT_BYTES {
            return Err(FileError::Invalid("file is too large for the text editor"));
        }
        let mut file = self
            .open(path, false, false, false)
            .await?
            .take(EDIT_BYTES + 1);
        let mut data = Vec::new();
        file.read_to_end(&mut data).await?;
        if data.len() as u64 > EDIT_BYTES {
            return Err(FileError::Invalid("file is too large for the text editor"));
        }
        String::from_utf8(data).map_err(|_| FileError::Invalid("file is not UTF-8 text"))
    }

    pub async fn write_text(&self, path: &str, text: &str, expected: Option<&str>) -> Result<()> {
        if text.len() as u64 > EDIT_BYTES {
            return Err(FileError::Invalid("text is too large"));
        }
        if let Some(expected) = expected {
            if self.read_text(path).await? != expected {
                return Err(FileError::Invalid(
                    "file changed since it was opened; reload before saving",
                ));
            }
        } else if self.exists(path).await?.is_some() {
            return Err(FileError::Exists(path.to_string()));
        }
        let mut file = self.open(path, true, true, expected.is_none()).await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await?;
        file.shutdown().await?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Conflict {
    #[default]
    Ask,
    Overwrite,
    Skip,
    Resume,
    Newer,
    Rename,
}

#[derive(Default)]
pub struct TransferControl {
    pub cancelled: AtomicBool,
    pub paused: AtomicBool,
    pub bytes_per_second: AtomicU64,
}

impl TransferControl {
    pub async fn check(&self) -> Result<()> {
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(FileError::Cancelled);
            }
            if !self.paused.load(Ordering::Relaxed) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub bytes: u64,
    pub total_bytes: u64,
    pub files: usize,
    pub total_files: usize,
    pub skipped: usize,
    pub current: String,
}

/// Copy local↔remote, remote↔remote or local↔local using bounded streaming.
/// Resume verifies the existing prefix before appending. Move deletes a source
/// only after the entire copy succeeds without skips.
#[expect(
    clippy::too_many_arguments,
    reason = "Both filesystem/path pairs, conflict and move policy, cancellation and progress belong to one transfer"
)]
pub async fn transfer(
    source: &FileSystem,
    from: &str,
    destination: &FileSystem,
    to: &str,
    conflict: Conflict,
    moving: bool,
    control: &TransferControl,
    report: impl Fn(Progress),
) -> Result<Progress> {
    let source_entry = source.metadata(from).await?;
    let canonical_source = if source_entry.symlink {
        let parent = Path::new(from)
            .parent()
            .and_then(Path::to_str)
            .ok_or(FileError::Invalid("source needs a parent"))?;
        child(&source.canonicalize(parent).await?, &source_entry.name)?
    } else {
        source.canonicalize(from).await?
    };
    let from = canonical_source.as_str();
    let same = match (source, destination) {
        (FileSystem::Local, FileSystem::Local) => true,
        (FileSystem::Remote(a), FileSystem::Remote(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    if same {
        source.check_destination(from, to).await?;
    }
    if moving && same && destination.exists(to).await?.is_none() {
        source.rename(from, to).await?;
        let progress = Progress {
            files: 1,
            total_files: 1,
            current: to.to_string(),
            ..Progress::default()
        };
        report(progress.clone());
        return Ok(progress);
    }
    let entries = source.walk(from, control).await?;
    let mut progress = Progress {
        total_bytes: entries
            .iter()
            .filter(|e| !e.directory && !e.symlink)
            .map(|e| e.size)
            .sum(),
        total_files: entries.len(),
        ..Progress::default()
    };
    report(progress.clone());
    // A renamed directory must carry its descendants to the same new root.
    let root = if conflict == Conflict::Rename {
        unique_destination(destination, to).await?
    } else {
        to.to_string()
    };
    for entry in entries {
        control.check().await?;
        let relative = entry
            .path
            .strip_prefix(from)
            .ok_or(FileError::Invalid("invalid recursive path"))?;
        let mut target = format!("{root}{relative}");
        progress.current = entry.path.clone();
        let mut existing = destination.exists(&target).await?;
        if conflict == Conflict::Rename && existing.is_some() && !entry.directory {
            target = unique_destination(destination, &target).await?;
            existing = None;
        }
        if entry.directory && !entry.symlink {
            match existing {
                Some(ref e) if e.directory && !e.symlink => {}
                Some(_) => return Err(FileError::Exists(target)),
                None => destination.mkdir(&target).await?,
            }
        } else {
            let skip = existing.as_ref().is_some_and(|e| {
                conflict == Conflict::Skip
                    || (conflict == Conflict::Newer && e.modified >= entry.modified)
            });
            if skip {
                progress.skipped += 1;
                progress.bytes += entry.size;
                progress.files += 1;
                report(progress.clone());
                continue;
            }
            if let Some(ref e) = existing
                && (conflict == Conflict::Ask || e.directory || e.symlink)
            {
                return Err(FileError::Exists(target));
            }
            if entry.symlink {
                if existing.is_some() {
                    return Err(FileError::Exists(target));
                }
                copy_link(source, &entry.path, destination, &target).await?;
            } else {
                copy_file(
                    source,
                    &entry,
                    destination,
                    &target,
                    existing.as_ref(),
                    conflict,
                    control,
                    &mut progress,
                    &report,
                )
                .await?;
            }
        }
        progress.files += 1;
        report(progress.clone());
    }
    if moving {
        if progress.skipped != 0 {
            return Err(FileError::Invalid(
                "move left source intact because some files were skipped",
            ));
        }
        source.remove(from, control).await?;
    }
    Ok(progress)
}

async fn unique_destination(fs: &FileSystem, path: &str) -> Result<String> {
    if fs.exists(path).await?.is_none() {
        return Ok(path.to_string());
    }
    for n in 1..=MAX_TREE_ENTRIES {
        let candidate = format!("{path} ({n})");
        if fs.exists(&candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(FileError::Invalid("no available destination name"))
}

#[allow(clippy::too_many_arguments)]
async fn copy_file(
    source: &FileSystem,
    entry: &Entry,
    destination: &FileSystem,
    target: &str,
    existing: Option<&Entry>,
    conflict: Conflict,
    control: &TransferControl,
    progress: &mut Progress,
    report: &impl Fn(Progress),
) -> Result<()> {
    let mut input = source.open(&entry.path, false, false, false).await?;
    let offset = if conflict == Conflict::Resume {
        existing.map_or(0, |e| e.size)
    } else {
        0
    };
    if offset > entry.size {
        return Err(FileError::Invalid(
            "destination is larger than source; cannot resume",
        ));
    }
    let mut buffer = vec![0; BUFFER_BYTES];
    if offset > 0 {
        let mut previous = destination.open(target, false, false, false).await?;
        let mut other = vec![0; BUFFER_BYTES];
        let mut checked = 0;
        while checked < offset {
            control.check().await?;
            let count = (offset - checked).min(BUFFER_BYTES as u64) as usize;
            input.read_exact(&mut buffer[..count]).await?;
            previous.read_exact(&mut other[..count]).await?;
            if buffer[..count] != other[..count] {
                return Err(FileError::Invalid(
                    "destination prefix differs; cannot resume",
                ));
            }
            checked += count as u64;
        }
    }
    let mut output = destination
        .open(
            target,
            true,
            conflict != Conflict::Resume,
            existing.is_none(),
        )
        .await?;
    output.seek(SeekFrom::Start(offset)).await?;
    progress.bytes += offset;
    let mut copied = offset;
    let mut reported = Instant::now();
    while copied < entry.size {
        control.check().await?;
        let started = Instant::now();
        let limit = (entry.size - copied).min(BUFFER_BYTES as u64) as usize;
        let count = input.read(&mut buffer[..limit]).await?;
        if count == 0 {
            return Err(FileError::Invalid("source shortened during transfer"));
        }
        output.write_all(&buffer[..count]).await?;
        copied += count as u64;
        progress.bytes += count as u64;
        let speed = control.bytes_per_second.load(Ordering::Relaxed);
        if speed > 0 {
            report(progress.clone());
            let desired = Duration::from_secs_f64(count as f64 / speed as f64);
            while started.elapsed() < desired {
                control.check().await?;
                tokio::time::sleep(
                    desired
                        .saturating_sub(started.elapsed())
                        .min(Duration::from_millis(100)),
                )
                .await;
            }
        }
        if reported.elapsed() >= Duration::from_millis(100) {
            report(progress.clone());
            reported = Instant::now();
        }
    }
    output.flush().await?;
    output.shutdown().await?;
    let after = source.metadata(&entry.path).await?;
    if after.size != entry.size || after.modified != entry.modified {
        return Err(FileError::Invalid("source changed during transfer"));
    }
    if let FileSystem::Remote(sftp) = destination {
        sftp.set_metadata(
            target,
            FileAttributes {
                atime: entry.modified,
                mtime: entry.modified,
                ..FileAttributes::empty()
            },
        )
        .await?;
    } else if let Some(modified) = entry.modified {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(target)
            .await?
            .into_std()
            .await;
        let time = std::time::UNIX_EPOCH + Duration::from_secs(u64::from(modified));
        tokio::task::spawn_blocking(move || {
            file.set_times(std::fs::FileTimes::new().set_modified(time))
        })
        .await
        .map_err(|_| FileError::Invalid("timestamp worker stopped"))??;
    }
    Ok(())
}

async fn copy_link(
    source: &FileSystem,
    from: &str,
    destination: &FileSystem,
    to: &str,
) -> Result<()> {
    let target = match source {
        FileSystem::Local => tokio::fs::read_link(from)
            .await?
            .to_string_lossy()
            .into_owned(),
        FileSystem::Remote(sftp) => sftp.read_link(from).await?,
    };
    match destination {
        FileSystem::Local => {
            #[cfg(unix)]
            {
                tokio::fs::symlink(target, to).await?;
            }
            #[cfg(not(unix))]
            {
                return Err(FileError::Invalid(
                    "local symbolic link transfer is unavailable",
                ));
            }
        }
        FileSystem::Remote(sftp) => sftp.symlink(to, target).await?,
    }
    Ok(())
}
