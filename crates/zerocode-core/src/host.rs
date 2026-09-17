//! 호스트 경계 — 한 워크스페이스의 파일과 git이 **어느 기계에** 있는가.
//!
//! [`Host`]의 실행 변종은 아직 [`Host::Local`] 하나뿐이고, 파일·git·PTY 동작도
//! 전부 로컬이다. P3부터는 그 변종을 늘리기 전에, renderer나 로컬 [`PathBuf`]가
//! 원격 주소를 대신하지 않도록 영속 경계의 값([`ExecutionHostId`], [`RemotePath`],
//! [`SshHostRecord`])부터 둔다. 실제 SSH transport가 붙기 전까지 이 값들은 로컬
//! 동작을 바꾸지 않는다.
//!
//! ## 왜 지금인가 — 실측된 반례
//!
//! Orca(Electron)의 SSH/원격 워크스페이스 서브시스템을 실측하면 원본 소스 기준
//! **3만~4만 줄**이고, 그 대부분이 기능이 아니라 분산 시스템 부기다. 그러나 이
//! 모듈이 존재하는 이유는 그 규모가 아니라 그 **모양**이다:
//!
//! - Orca에는 `trait GitProvider` 하나가 없다. 대신
//!   `if (context.connectionId) { …ssh… } else { …local… }` 꼴의 인라인 분기가
//!   메인 프로세스 한 파일에만 **737곳** 흩어져 있다
//!   (`getSshGitProvider(` 112회, `getSshFilesystemProvider(` 62회,
//!   `requireSshFilesystemProvider(` 18회, `requireSshGitProvider(` 10회,
//!   `getSshPtyProvider(` 8회).
//! - 그래서 원격에서만 빠지는 기능이 생기고, 그 구멍은 어느 한 곳을 고쳐서
//!   메울 수가 없다. 737곳 중 어디를 빠뜨렸는지 컴파일러가 말해 주지 않는다.
//!
//! ZeroCode는 원격 코드가 0줄인 지금 그 반대편에 선다. **호출부는 [`Host`]를
//! 들고 다니고, 어느 변종인지 묻지 않는다.** 무엇을 어디서 할지는 오직 이
//! 파일의 `match`가 정한다.
//!
//! ## 그래서 이 파일의 계약은 셋이다
//!
//! 1. **결정은 한 곳이다.** 어떤 워크스페이스가 어느 호스트에 있는지는
//!    [`Host::for_workspace`]만 답한다. 워크스페이스가 연결을 이름 부를 수 있게
//!    되는 날, 읽는 곳은 그 함수 하나다.
//! 2. **해소도 한 곳이다.** [`Host::fs`]와 [`Host::vcs`]의 `match`가 유일한
//!    분기다. [`Host`]에 변종이 하나 늘면 그 `match`들이 **컴파일 에러**가 되고,
//!    컴파일러가 고쳐야 할 자리를 전부 나열해 준다 — 737곳을 사람이 세는 것과
//!    이것이 다른 점이다.
//! 3. **호출부는 변종을 이름 부르지 않는다.** `Host::Local`이라는 글자가
//!    호출부에 나타나는 순간 그것이 Orca의 첫 번째 `if`다. 창의 게이트가 그
//!    글자를 금지한다.
//!
//! ## 왜 동기(sync)인가
//!
//! 실측 보고의 스케치는 `#[async_trait]`이지만 이 저장소의 실제 호출 패턴이
//! 아니다. 여기 있는 연산들은 전부 `spawn_blocking` 안에서 동기로 불린다(창의
//! `#[tauri::command]`들이 그렇게 감싼다). 경계를 async로 만들면 그 호출부
//! 전부를 지금 바꿔야 하고, 그것은 "원격 코드 0줄"이라는 이 단계의 전제를
//! 깨뜨린다. 원격 impl은 자기 런타임 핸들 위에서 `block_on` 하면 되고, 그
//! 자리는 이미 블로킹 스레드다 — 잃는 것이 없다.
//!
//! ## 무엇이 여기 없는가 — PTY
//!
//! PTY도 워크스페이스 단위로 원격화될 표면이지만 이 크레이트에 있을 수 없다.
//! `zerocode-pty`는 `zerocode-core` **아래가 아니라 옆**이라, 여기서는 살아 있는
//! 터미널 핸들(`PtyLane`)을 이름 부를 수 없다. 그래서 경계를 넘는 **값**만
//! 여기 있고([`PtySpec`]), 면 자체는 `zerocode-lane`이 확장 trait으로 붙인다.
//! 스위치는 그래도 하나다 — 거기서도 `match`하는 것은 이 [`Host`]다.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Stable identity of a machine where work is executed.
///
/// Labels, addresses and credentials can all change without changing this
/// value. Persisted workspaces therefore point at an identity rather than at
/// whichever endpoint spelling happened to be current when they were saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionHostId(Uuid);

impl ExecutionHostId {
    /// Allocate an identity for a newly confirmed host.
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    /// The UUID used on the persistence wire.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ExecutionHostId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ExecutionHostId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Why a path cannot be sent to a POSIX remote host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePathError {
    Empty,
    NotAbsolute,
    ExpectedRelative,
    ContainsNul,
    EmptySegment,
    CurrentDirectorySegment,
    ParentDirectorySegment,
}

impl fmt::Display for RemotePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "remote path is empty",
            Self::NotAbsolute => "remote path is not absolute",
            Self::ExpectedRelative => "workspace path must be relative",
            Self::ContainsNul => "remote path contains NUL",
            Self::EmptySegment => "remote path contains an empty segment",
            Self::CurrentDirectorySegment => "remote path contains a current-directory segment",
            Self::ParentDirectorySegment => "remote path contains a parent-directory segment",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RemotePathError {}

/// An absolute, canonical-by-construction UTF-8 POSIX path.
///
/// This is deliberately not a [`PathBuf`]. A local path can carry platform
/// separators and non-UTF-8 bytes; an SSH/SFTP path cannot silently inherit
/// either property. Deserialization calls the same validator as construction,
/// so a hand-edited settings file cannot bypass the invariant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemotePath(String);

impl RemotePath {
    pub fn parse(value: impl Into<String>) -> Result<Self, RemotePathError> {
        let value = value.into();
        validate_absolute_remote_path(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve a workspace-relative path without permitting lexical escape.
    ///
    /// The remote backend still realpaths the workspace and existing target;
    /// this check is the earlier boundary that prevents an absolute path or
    /// `..` from ever reaching that backend as a workspace request.
    pub fn join_relative(&self, relative: &str) -> Result<Self, RemotePathError> {
        validate_relative_remote_path(relative)?;
        if relative.is_empty() {
            return Ok(self.clone());
        }
        let joined = if self.0 == "/" {
            format!("/{relative}")
        } else {
            format!("{}/{relative}", self.0)
        };
        Self::parse(joined)
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RemotePath {
    type Err = RemotePathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for RemotePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RemotePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

fn validate_absolute_remote_path(value: &str) -> Result<(), RemotePathError> {
    if value.is_empty() {
        return Err(RemotePathError::Empty);
    }
    if value.contains('\0') {
        return Err(RemotePathError::ContainsNul);
    }
    let Some(relative) = value.strip_prefix('/') else {
        return Err(RemotePathError::NotAbsolute);
    };
    if relative.is_empty() {
        return Ok(());
    }
    validate_remote_segments(relative)
}

fn validate_relative_remote_path(value: &str) -> Result<(), RemotePathError> {
    if value.contains('\0') {
        return Err(RemotePathError::ContainsNul);
    }
    if value.starts_with('/') {
        return Err(RemotePathError::ExpectedRelative);
    }
    if value.is_empty() {
        return Ok(());
    }
    validate_remote_segments(value)
}

fn validate_remote_segments(value: &str) -> Result<(), RemotePathError> {
    for segment in value.split('/') {
        match segment {
            "" => return Err(RemotePathError::EmptySegment),
            "." => return Err(RemotePathError::CurrentDirectorySegment),
            ".." => return Err(RemotePathError::ParentDirectorySegment),
            _ => {}
        }
    }
    Ok(())
}

/// Stable categories shared by remote files, Git and PTY transports.
///
/// Provider text stays behind the transport boundary; renderer-facing code
/// maps this finite set to local copy instead of forwarding arbitrary SSH
/// diagnostics (which can contain endpoints, usernames or command output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    Missing,
    NoAccess,
    Transport,
    Authentication,
    Timeout,
    Cancelled,
    InvalidPath(RemotePathError),
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("execution-host resource is missing"),
            Self::NoAccess => formatter.write_str("execution-host access was denied"),
            Self::Transport => formatter.write_str("execution-host transport failed"),
            Self::Authentication => formatter.write_str("execution-host authentication failed"),
            Self::Timeout => formatter.write_str("execution-host operation timed out"),
            Self::Cancelled => formatter.write_str("execution-host operation was cancelled"),
            Self::InvalidPath(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPath(error) => Some(error),
            _ => None,
        }
    }
}

/// Standard SSH port, kept here so callers do not each invent a default.
pub const SSH_DEFAULT_PORT: u16 = 22;

/// Why an SSH endpoint or pinned public key is not safe to persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshHostError {
    EmptyHost,
    InvalidHost,
    EmptyUser,
    InvalidUser,
    InvalidPort,
    EmptyKeyAlgorithm,
    InvalidKeyAlgorithm,
    EmptyEncodedKey,
    InvalidEncodedKey,
}

impl fmt::Display for SshHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyHost => "SSH host is empty",
            Self::InvalidHost => "SSH host contains whitespace or control characters",
            Self::EmptyUser => "SSH user is empty",
            Self::InvalidUser => "SSH user contains whitespace or control characters",
            Self::InvalidPort => "SSH port must be non-zero",
            Self::EmptyKeyAlgorithm => "SSH host-key algorithm is empty",
            Self::InvalidKeyAlgorithm => {
                "SSH host-key algorithm contains whitespace or control characters"
            }
            Self::EmptyEncodedKey => "SSH host key is empty",
            Self::InvalidEncodedKey => "SSH host key is not one encoded field",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SshHostError {}

/// A network endpoint and login name. It never contains a password or key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SshEndpoint {
    host: String,
    port: u16,
    user: String,
}

impl SshEndpoint {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
    ) -> Result<Self, SshHostError> {
        let host = host.into();
        let user = user.into();
        validate_ssh_atom(&host, SshHostError::EmptyHost, SshHostError::InvalidHost)?;
        validate_ssh_atom(&user, SshHostError::EmptyUser, SshHostError::InvalidUser)?;
        if port == 0 {
            return Err(SshHostError::InvalidPort);
        }
        Ok(Self { host, port, user })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SshEndpointWire {
    host: String,
    port: u16,
    user: String,
}

impl<'de> Deserialize<'de> for SshEndpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SshEndpointWire::deserialize(deserializer)?;
        Self::new(wire.host, wire.port, wire.user).map_err(serde::de::Error::custom)
    }
}

/// The only credential choices in the first SSH vertical slice.
///
/// Password bytes live in the native credential store under the host identity;
/// this enum is safe to persist and send to the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthentication {
    Agent,
    Password,
}

/// Exact SSH public key confirmed by the user for one endpoint.
///
/// This stores the wire key, not merely a human-readable fingerprint, so the
/// transport can compare the server key byte for byte on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PinnedHostKey {
    algorithm: String,
    encoded_key: String,
}

impl PinnedHostKey {
    pub fn new(
        algorithm: impl Into<String>,
        encoded_key: impl Into<String>,
    ) -> Result<Self, SshHostError> {
        let algorithm = algorithm.into();
        let encoded_key = encoded_key.into();
        validate_ssh_atom(
            &algorithm,
            SshHostError::EmptyKeyAlgorithm,
            SshHostError::InvalidKeyAlgorithm,
        )?;
        validate_ssh_atom(
            &encoded_key,
            SshHostError::EmptyEncodedKey,
            SshHostError::InvalidEncodedKey,
        )?;
        Ok(Self {
            algorithm,
            encoded_key,
        })
    }

    #[must_use]
    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    #[must_use]
    pub fn encoded_key(&self) -> &str {
        &self.encoded_key
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedHostKeyWire {
    algorithm: String,
    encoded_key: String,
}

impl<'de> Deserialize<'de> for PinnedHostKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PinnedHostKeyWire::deserialize(deserializer)?;
        Self::new(wire.algorithm, wire.encoded_key).map_err(serde::de::Error::custom)
    }
}

fn validate_ssh_atom(
    value: &str,
    empty: SshHostError,
    invalid: SshHostError,
) -> Result<(), SshHostError> {
    if value.is_empty() {
        return Err(empty);
    }
    if value.chars().any(char::is_whitespace) || value.chars().any(char::is_control) {
        return Err(invalid);
    }
    Ok(())
}

/// An endpoint draft has no trusted server identity yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostDraft {
    id: ExecutionHostId,
    endpoint: SshEndpoint,
    authentication: SshAuthentication,
}

impl SshHostDraft {
    #[must_use]
    pub const fn new(
        id: ExecutionHostId,
        endpoint: SshEndpoint,
        authentication: SshAuthentication,
    ) -> Self {
        Self {
            id,
            endpoint,
            authentication,
        }
    }

    #[must_use]
    pub const fn id(&self) -> ExecutionHostId {
        self.id
    }

    #[must_use]
    pub const fn endpoint(&self) -> &SshEndpoint {
        &self.endpoint
    }

    #[must_use]
    pub const fn authentication(&self) -> SshAuthentication {
        self.authentication
    }

    #[must_use]
    pub fn confirm(self, pinned_host_key: PinnedHostKey) -> SshHostRecord {
        SshHostRecord {
            id: self.id,
            endpoint: self.endpoint,
            authentication: self.authentication,
            pinned_host_key,
        }
    }
}

/// A persisted SSH host whose server public key was explicitly confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostRecord {
    id: ExecutionHostId,
    endpoint: SshEndpoint,
    authentication: SshAuthentication,
    pinned_host_key: PinnedHostKey,
}

impl SshHostRecord {
    #[must_use]
    pub const fn id(&self) -> ExecutionHostId {
        self.id
    }

    #[must_use]
    pub const fn endpoint(&self) -> &SshEndpoint {
        &self.endpoint
    }

    #[must_use]
    pub const fn authentication(&self) -> SshAuthentication {
        self.authentication
    }

    #[must_use]
    pub const fn pinned_host_key(&self) -> &PinnedHostKey {
        &self.pinned_host_key
    }

    /// Editing the endpoint consumes the verified record and returns a draft.
    /// Re-confirmation is therefore mandatory before it can be persisted again.
    #[must_use]
    pub fn change_endpoint(self, endpoint: SshEndpoint) -> SshHostDraft {
        SshHostDraft::new(self.id, endpoint, self.authentication)
    }

    /// The same verified host, asked to authenticate with a password instead.
    ///
    /// This exists for one road: an agent host whose agent is unreachable can
    /// still be entered by a password the person types at that moment —
    /// Orca's own fallback ("an agent socket failure can still be recovered
    /// by password auth", `ssh-connection.ts`). Unlike
    /// [`Self::change_endpoint`] it returns a record, not a draft, because
    /// the endpoint and the pinned server key — the two facts confirmation
    /// vouched for — are exactly what it keeps.
    #[must_use]
    pub fn with_password_authentication(self) -> SshHostRecord {
        SshHostRecord {
            authentication: SshAuthentication::Password,
            ..self
        }
    }
}

/// A workspace location on a verified execution host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteWorkspace {
    pub host_id: ExecutionHostId,
    pub root: RemotePath,
}

/// 이 워크스페이스가 사는 기계.
///
/// 지금은 변종이 하나다. 그것이 이 타입의 값어치이기도 하다 — 변종이 둘이 되는
/// 날 컴파일러가 [`Host::fs`]·[`Host::vcs`]와 `zerocode-lane`의 PTY 해소기를
/// 전부 짚어 준다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// 이 프로세스가 도는 기계.
    Local,
}

impl Host {
    /// 이 워크스페이스는 어느 기계에 있는가.
    ///
    /// **이 저장소에서 그 물음에 답하는 유일한 함수다.** 오늘의 답은 언제나
    /// [`Host::Local`]이고, 워크스페이스가 원격 연결을 이름 부를 수 있게 되면
    /// 그 이름을 읽는 곳은 여기 하나다. 호출부가 저마다 읽기 시작하는 것이
    /// 정확히 Orca가 간 길이다.
    pub fn for_workspace(_root: &Path) -> Self {
        Host::Local
    }

    /// 이 호스트의 파일 접근.
    pub fn fs(&self) -> &dyn Fs {
        match self {
            Host::Local => &LocalFs,
        }
    }

    /// 이 호스트의 git 접근.
    pub fn vcs(&self) -> &dyn Vcs {
        match self {
            Host::Local => &LocalVcs,
        }
    }
}

/// 목록에 오른 항목 하나에 대해 **읽어 낸** 사실.
///
/// 링크를 따라가지 않고 읽은 것이다([`Fs::read_dir`] 참고).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct About {
    /// 디렉터리인가. 링크는 링크이지 그것이 가리키는 것이 아니다.
    pub is_dir: bool,
    /// 이 항목이 **디스크에서** 차지하는 바이트.
    ///
    /// 논리 크기가 아니라 점유 블록이다 — `du`가 세는 값이고, "지우면 얼마가
    /// 돌아오나"의 답이기도 하다. 1바이트짜리 파일 만 개는 논리로 10KB이고
    /// 디스크에서는 40MB다.
    pub disk_bytes: u64,
}

/// 디렉터리 목록의 한 줄.
///
/// `about`이 [`None`]인 것은 **목록에는 있는데 설명을 읽지 못했다**는 뜻이고,
/// 목록에 없는 것과 다르다. 순회는 그런 줄도 자기 상한에 센다 — 셀 수 없는
/// 항목을 세지 않으면 상한이 조용히 헐거워진다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// 이 호스트가 쓰는 철자 그대로의 경로.
    pub path: PathBuf,
    /// 읽어 낸 사실, 또는 읽지 못했음.
    pub about: Option<About>,
}

impl DirEntry {
    /// 마지막 마디. 경로에 마디가 없으면 빈 문자열이다.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|one| one.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// 디렉터리를 열지 못한 이유 — 셋뿐이고, 셋이 서로 다른 화면을 만든다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDir {
    /// 없다. 흔한 상태이고(손으로 지운 체크아웃), 실패가 아니라 그 자체가 사실이다.
    Missing,
    /// 있는데 열 권한이 없다.
    NoAccess,
    /// 그 밖의 전부.
    Failed,
}

/// 이 호스트의 파일 — **지금 실제로 쓰이는 네 연산뿐**이다.
///
/// 상상해서 늘리지 않는다. 여기 있는 넷은 전부 오늘 창이 부르고 있는 것이고,
/// 다섯 번째는 그것을 부르는 호출부가 이 경계로 넘어오는 날 함께 온다.
pub trait Fs: Send + Sync {
    /// 한 디렉터리의 항목 전부와, 각각에 대해 읽어 낸 사실.
    ///
    /// **링크를 따라가지 않는다.** 따라가면 워크트리 밖의 바이트를 이
    /// 워크스페이스의 것으로 세고, 링크가 자기 위를 가리키면 순회가 끝나지
    /// 않는다.
    ///
    /// 원격을 위한 모양이기도 하다. 목록과 각 항목의 사실이 **한 번에** 온다 —
    /// 항목마다 왕복하면 만 개짜리 디렉터리가 만 번의 왕복이다.
    fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, ReadDir>;

    /// 이것은 디렉터리인가. 읽지 못하면 `false`.
    fn is_dir(&self, path: &Path) -> bool;

    /// 파일 전체를 문자열로. 읽지 못하면 [`None`].
    fn read_to_string(&self, path: &Path) -> Option<String>;

    /// 파일이 마지막으로 바뀐 시각, epoch 밀리초. 읽지 못하면 [`None`] — 0이 아니다.
    fn modified_ms(&self, path: &Path) -> Option<i64>;
}

/// 예산 안에서 돌린 git이 한 말 — **셋으로 편 것**.
///
/// [`Vcs::text_within`]이 [`None`] 하나로 접는 것을 갈라 놓는다. git에게는
/// "원격에 그 ref가 없다"와 "토큰이 만료됐다"와 "네트워크가 끊겼다"가 전부 0이
/// 아닌 종료값이지만, 부른 쪽에게는 각각 **폴백·거절·이미 가진 것 재사용**이다.
/// 실패의 이유가 다음 결정을 바꾸는 자리에서만 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Within {
    /// git이 0으로 끝났다 — 표준 출력.
    Said(String),
    /// git이 0이 아닌 값으로 끝났다 — 표준 오류에 적은 말.
    Refused(String),
    /// 답이 없다 — 예산 안에 끝나지 않아 죽였거나, 띄우지도 못했다.
    Silent,
}

/// 이 호스트의 git — **지금 실제로 쓰이는 세 모양뿐**이다.
///
/// 셋은 편의가 아니라 서로 다른 실패 모드다. [`Vcs::text`]는 git이 한 말을
/// 그대로 사람에게 전하고, [`Vcs::text_within`]은 **답을 못 받았음**을 빈
/// 문자열과 구별해서 돌려주며, [`Vcs::try_within`]은 그 위에 **왜 못 받았는지**를
/// 얹는다. 하나로 합치면 셋 중 둘을 잃는다.
pub trait Vcs: Send + Sync {
    /// `root`에서 git을 돌리고 표준 출력을 준다. 실패하면 git이 표준 오류에
    /// 적은 말을 그대로 준다.
    ///
    /// `!status.success()`를 잊은 복사본은 스테이지가 빈 것과 명령이 실패한
    /// 것을 같은 빈 문자열로 답한다. 그 실패를 막으려고 이 함수가 하나다.
    fn text(&self, root: &Path, args: &[&str]) -> Result<String, String>;

    /// 같은 일을 하되 `budget` 안에 끝나지 않으면 **죽인다**. 실패의 이유까지
    /// 준다 — [`Within`]을 볼 것.
    ///
    /// 기다리기를 그만두는 것과 죽이는 것은 다르다 — 앞의 것은 창이 사는 동안
    /// 매달려 있는 git을 하나 남긴다.
    fn try_within(&self, root: &Path, args: &[&str], budget: Duration) -> Within;

    /// 같은 일을 하되 **이유를 묻지 않는** 자리를 위한 얼굴.
    ///
    /// [`None`]은 "답을 받지 못했다"이고, 빈 문자열과 다르다. 셋 중 하나만 답이고
    /// 나머지 둘은 같은 [`None`]으로 접힌다 — 접어도 되는 자리는 실패해도 다음
    /// 수가 같은 자리뿐이다(예: 가져오지 못해도 이미 가진 ref로 답이 되는 비교
    /// 기준). 이유가 다음 수를 바꾸면 [`Vcs::try_within`]을 쓴다.
    fn text_within(&self, root: &Path, args: &[&str], budget: Duration) -> Option<String> {
        match self.try_within(root, args, budget) {
            Within::Said(said) => Some(said),
            Within::Refused(_) | Within::Silent => None,
        }
    }

    /// `url`을 `into`로 받아 온다. 진행률은 `say`로 흘려 보낸다.
    ///
    /// 셋째 모양인 이유는 앞의 둘과 **실패 모드가 또 다르기** 때문이다.
    /// [`Vcs::text`]는 끝난 뒤에 한 번 말하고, 이것은 **도는 동안** 말한다 —
    /// 클론은 분 단위이고, 그동안 아무 말도 없는 화면은 멈춘 화면과 구별되지
    /// 않는다.
    ///
    /// 계약 셋:
    ///
    ///   1. **비대화형이다.** 자격 증명을 물어야 하는 저장소는 터미널이 없는
    ///      자식 안에서 **묻다가 매달리는** 대신 그 자리에서 실패한다.
    ///   2. **실패는 git의 마지막 문장이다.** 그리고 그 문장은
    ///      [`clone_failure`](crate::clone::clone_failure)를 지나 자격
    ///      증명이 지워진 채 돌아온다.
    ///   3. **부분 클론은 이 함수가 치우지 않는다.** 무엇을 지워도 되는지는
    ///      "그 디렉터리를 누가 만들었는가"의 답이고, 그것을 아는 것은 부른
    ///      쪽이다. 여기서 지우면 이미 있던 폴더를 지우는 날이 온다.
    fn clone(
        &self,
        url: &str,
        into: &Path,
        say: &mut dyn FnMut(crate::clone::CloneStep),
    ) -> Result<(), String>;
}

/// PTY 작업 디렉터리가 해석되는 기계.
///
/// 이 구분이 있어야 검증된 원격 POSIX 경로가 로컬 [`PathBuf`]로 약화되지
/// 않는다. 로컬 실행기는 [`Self::Local`]만, 원격 transport는 [`Self::Remote`]만
/// 받아야 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyCwd {
    Local(PathBuf),
    Remote(RemotePath),
}

impl PtyCwd {
    #[must_use]
    pub fn local(&self) -> Option<&Path> {
        match self {
            Self::Local(path) => Some(path),
            Self::Remote(_) => None,
        }
    }

    #[must_use]
    pub const fn remote(&self) -> Option<&RemotePath> {
        match self {
            Self::Local(_) => None,
            Self::Remote(path) => Some(path),
        }
    }
}

/// PTY 하나를 띄우라는 요청 — 경계를 넘는 **값**.
///
/// 이 여섯이 오늘 모든 PTY 호출부가 짓는 것 전부이고, 원격 `pty.spawn` RPC가
/// 실어 보낼 것도 이 여섯이다. 살아 있는 핸들 쪽(재생·ACK·크레딧)은 프로토콜이
/// 정하는 모양이라 지금 지어내지 않는다 — 그것이 이 단계가 원격 코드를 한 줄도
/// 쓰지 않는다는 말의 뜻이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySpec {
    /// 띄울 프로그램.
    pub program: String,
    /// 인자.
    pub args: Vec<String>,
    /// 작업 디렉터리와 그것을 해석할 기계. 없으면 부모의 것을 물려받는다.
    pub cwd: Option<PtyCwd>,
    /// 물려받은 환경 **위에** 얹는 것들.
    ///
    /// 값이 **비어 있으면 "이것을 지워라"**이지 "빈 값으로 두어라"가 아니다.
    /// 존재를 검사하는 쪽에게 그 둘은 다르고, 계정 전환이 자격 증명 변수를
    /// 지울 수 있는 것이 그 차이 덕분이다.
    pub env: Vec<(String, String)>,
    /// 처음 크기.
    pub rows: u16,
    /// 처음 크기.
    pub cols: u16,
}

impl PtySpec {
    /// 오늘의 호출부가 들고 있는 여섯 값을 그대로 묶는다.
    pub fn new(
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Self {
        Self {
            program: program.to_string(),
            args: args.to_vec(),
            cwd: cwd.map(|path| PtyCwd::Local(path.to_path_buf())),
            env: env.to_vec(),
            rows,
            cols,
        }
    }

    /// 검증된 POSIX 작업 디렉터리에서 원격 PTY를 띄울 요청을 만든다.
    #[must_use]
    pub fn remote(
        program: &str,
        args: &[String],
        cwd: RemotePath,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Self {
        Self {
            program: program.to_string(),
            args: args.to_vec(),
            cwd: Some(PtyCwd::Remote(cwd)),
            env: env.to_vec(),
            rows,
            cols,
        }
    }
}

/// 이 프로세스가 도는 기계의 파일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalFs;

impl Fs for LocalFs {
    fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, ReadDir> {
        let listing = std::fs::read_dir(dir).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => ReadDir::Missing,
            std::io::ErrorKind::PermissionDenied => ReadDir::NoAccess,
            _ => ReadDir::Failed,
        })?;
        Ok(listing
            .flatten()
            .map(|found| {
                let path = found.path();
                // `symlink_metadata`인 것이 이 경계의 계약이다. 링크를 따라가는
                // 순간 이 워크스페이스는 남의 바이트를 자기 것으로 세기
                // 시작한다.
                let about = path.symlink_metadata().ok().map(|about| About {
                    is_dir: about.is_dir(),
                    disk_bytes: disk_bytes(&about),
                });
                DirEntry { path, about }
            })
            .collect())
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn modified_ms(&self, path: &Path) -> Option<i64> {
        let changed = std::fs::metadata(path).ok()?.modified().ok()?;
        let since = changed.duration_since(std::time::UNIX_EPOCH).ok()?;
        i64::try_from(since.as_millis()).ok()
    }
}

/// 한 항목이 디스크에서 차지하는 바이트. 유닉스에서는 점유 블록이고, 그
/// 개념이 없는 곳에서는 논리 길이다.
#[cfg(unix)]
fn disk_bytes(about: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    about.blocks() * 512
}

#[cfg(not(unix))]
fn disk_bytes(about: &std::fs::Metadata) -> u64 {
    about.len()
}

/// 이 프로세스가 도는 기계의 git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalVcs;

impl Vcs for LocalVcs {
    fn text(&self, root: &Path, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|error| error.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn try_within(&self, root: &Path, args: &[&str], budget: Duration) -> Within {
        let Ok(mut child) = Command::new("git")
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // 표준 오류는 더 이상 버리지 않는다. 버리면 남는 것은 "0이 아니었다"
            // 하나뿐이고, 그 한 낱말로는 폴백할 실패와 거절할 실패를 가를 수 없다.
            .stderr(Stdio::piped())
            .spawn()
        else {
            return Within::Silent;
        };
        // 파이프는 **각자** 자기 스레드가 비운다. `--untracked-files=all`의 출력은
        // 큰 체크아웃에서 파이프 버퍼를 가볍게 넘고, 자식이 끝나기를 기다린 뒤에
        // 읽으면 그 순간 교착이다 — 시간 초과로 "실패"라고 답하게 되는데,
        // 실패한 것은 git이 아니라 이 함수다. 표준 오류도 같은 이유로 같은
        // 대접을 받는다(진행률을 쏟는 fetch가 그 자리에 선다).
        let (Some(mut pipe), Some(mut complaint)) = (child.stdout.take(), child.stderr.take())
        else {
            return Within::Silent;
        };
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut said);
            said
        });
        let listening = std::thread::spawn(move || {
            let mut complained = Vec::new();
            let _ = std::io::Read::read_to_end(&mut complaint, &mut complained);
            complained
        });
        let began = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if began.elapsed() >= budget {
                        // 기다리기를 그만두는 것과 죽이는 것은 다르다. 앞의
                        // 것은 창이 사는 동안 매달려 있는 git을 하나 남긴다.
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = draining.join();
                        let _ = listening.join();
                        return Within::Silent;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return Within::Silent,
            }
        };
        let (Ok(said), Ok(complained)) = (draining.join(), listening.join()) else {
            return Within::Silent;
        };
        if status.success() {
            Within::Said(String::from_utf8_lossy(&said).into_owned())
        } else {
            Within::Refused(String::from_utf8_lossy(&complained).trim().to_string())
        }
    }

    fn clone(
        &self,
        url: &str,
        into: &Path,
        say: &mut dyn FnMut(crate::clone::CloneStep),
    ) -> Result<(), String> {
        let parent = into
            .parent()
            .ok_or_else(|| "복제할 위치가 없습니다".to_string())?;
        let mut child = Command::new("git")
            // `--`가 장식이 아닌 이유: `-` 로 시작하는 URL을 사람이 붙여넣으면
            // git은 그것을 플래그로 읽는다.
            .args(["clone", "--progress", "--"])
            .arg(url)
            .arg(into)
            .current_dir(parent)
            // 비대화형. 터미널이 없는 자식에게 자격 증명을 묻게 두면 클론은
            // 실패하는 대신 **매달린다** — 밖에서는 "0%에서 멈췄다"로 보인다.
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let mut pipe = child
            .stderr
            .take()
            .ok_or_else(|| "git이 말할 통로가 없습니다".to_string())?;
        // 이 스레드가 파이프를 비우는 것이 진행률의 전부다. 자식이 끝나기를
        // 먼저 기다리면 큰 저장소에서 파이프가 차서 교착이고, 교착은 밖에서
        // "멈춘 클론"으로 보인다 — `text_within`이 자기 스레드를 쓰는 것과
        // 같은 이유이고, 여기서는 읽은 것을 곧바로 쓰기까지 한다.
        let mut said = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = match std::io::Read::read(&mut pipe, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            // 진행률만 손실을 감수하고 읽는다: 덩어리 경계가 문자를 가르면 그
            // 한 순간이 안 읽힐 뿐이고, 다음 덩어리가 곧 온다. 모아 두는
            // `said`는 바이트로 남겨 두었다가 끝에서 한 번에 문자열이 된다.
            if let Some(step) =
                crate::clone::read_progress(&String::from_utf8_lossy(&buffer[..read]))
            {
                say(step);
            }
            said.extend_from_slice(&buffer[..read]);
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            return Ok(());
        }
        Err(crate::clone::clone_failure(&String::from_utf8_lossy(&said))
            .unwrap_or_else(|| "복제하지 못했습니다".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_host_identity_survives_the_persistence_wire() {
        let identity = ExecutionHostId::generate();
        let encoded = serde_json::to_string(&identity).expect("serialize host identity");
        let decoded: ExecutionHostId =
            serde_json::from_str(&encoded).expect("deserialize host identity");

        assert_eq!(decoded, identity);
        assert_eq!(identity.to_string().parse(), Ok(identity));
    }

    #[test]
    fn remote_paths_are_absolute_posix_paths_on_every_local_platform() {
        let root = RemotePath::parse("/").expect("remote root");
        let project = RemotePath::parse("/srv/work/zerocode").expect("remote project");

        assert_eq!(root.as_str(), "/");
        assert_eq!(project.as_str(), "/srv/work/zerocode");
        assert_eq!(project.to_string(), "/srv/work/zerocode");
        let encoded = serde_json::to_string(&project).expect("serialize remote path");
        let decoded: RemotePath = serde_json::from_str(&encoded).expect("deserialize remote path");
        assert_eq!(decoded, project);
    }

    #[test]
    fn remote_paths_reject_ambiguous_or_escaping_spellings() {
        for (path, expected) in [
            ("", RemotePathError::Empty),
            ("srv/work", RemotePathError::NotAbsolute),
            ("/srv//work", RemotePathError::EmptySegment),
            ("/srv/./work", RemotePathError::CurrentDirectorySegment),
            ("/srv/../work", RemotePathError::ParentDirectorySegment),
            ("/srv/work/", RemotePathError::EmptySegment),
            ("/srv/\0work", RemotePathError::ContainsNul),
        ] {
            assert_eq!(RemotePath::parse(path), Err(expected), "accepted {path:?}");
        }
        assert!(
            serde_json::from_str::<RemotePath>(r#""../outside""#).is_err(),
            "deserialization bypassed the constructor"
        );
    }

    #[test]
    fn a_workspace_relative_path_cannot_escape_its_remote_root() {
        let root = RemotePath::parse("/srv/work").expect("workspace root");

        assert_eq!(
            root.join_relative("src/lib.rs")
                .expect("workspace child")
                .as_str(),
            "/srv/work/src/lib.rs"
        );
        assert_eq!(root.join_relative("").expect("workspace itself"), root);
        assert_eq!(
            root.join_relative("/etc/passwd"),
            Err(RemotePathError::ExpectedRelative)
        );
        assert_eq!(
            root.join_relative("../outside"),
            Err(RemotePathError::ParentDirectorySegment)
        );
        assert_eq!(
            root.join_relative("src//lib.rs"),
            Err(RemotePathError::EmptySegment)
        );
    }

    #[test]
    fn pty_specs_keep_local_and_remote_working_directories_distinct() {
        let args = vec!["--flag".to_string()];
        let env = vec![("TERM".to_string(), "xterm-256color".to_string())];
        let local = PtySpec::new("tool", &args, Some(Path::new("/tmp/work")), &env, 24, 80);
        assert_eq!(
            local.cwd.as_ref().and_then(PtyCwd::local),
            Some(Path::new("/tmp/work"))
        );
        assert!(local.cwd.as_ref().and_then(PtyCwd::remote).is_none());

        let root = RemotePath::parse("/srv/work").expect("validated remote root");
        let remote = PtySpec::remote("tool", &args, root.clone(), &env, 24, 80);
        assert_eq!(remote.cwd.as_ref().and_then(PtyCwd::remote), Some(&root));
        assert!(remote.cwd.as_ref().and_then(PtyCwd::local).is_none());
        assert_eq!(
            (local.program, local.args, local.env, local.rows, local.cols),
            (
                remote.program,
                remote.args,
                remote.env,
                remote.rows,
                remote.cols
            )
        );
    }

    #[test]
    fn ssh_endpoint_and_pin_deserialization_cannot_bypass_validation() {
        let endpoint =
            SshEndpoint::new("dev.example.test", SSH_DEFAULT_PORT, "joe").expect("valid endpoint");
        let pin =
            PinnedHostKey::new("ssh-ed25519", "AAAAC3NzaC1lZDI1NTE5AAAAITest").expect("valid key");

        assert_eq!(
            serde_json::from_str::<SshEndpoint>(
                &serde_json::to_string(&endpoint).expect("serialize endpoint")
            )
            .expect("deserialize endpoint"),
            endpoint
        );
        assert_eq!(
            serde_json::from_str::<PinnedHostKey>(
                &serde_json::to_string(&pin).expect("serialize key")
            )
            .expect("deserialize key"),
            pin
        );
        assert!(
            serde_json::from_str::<SshEndpoint>(
                r#"{"host":"dev.example.test\nother","port":22,"user":"joe"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<PinnedHostKey>(
                r#"{"algorithm":"ssh-ed25519","encoded_key":"key other"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn changing_an_endpoint_clears_the_old_host_key_by_construction() {
        let id = ExecutionHostId::generate();
        let original = SshEndpoint::new("old.example.test", SSH_DEFAULT_PORT, "joe")
            .expect("original endpoint");
        let pin = PinnedHostKey::new("ssh-ed25519", "AAAAC3NzaOld").expect("original host key");
        let record = SshHostDraft::new(id, original, SshAuthentication::Agent).confirm(pin.clone());
        let encoded = serde_json::to_string(&record).expect("serialize verified host");
        let decoded: SshHostRecord =
            serde_json::from_str(&encoded).expect("deserialize verified host");
        assert_eq!(decoded, record);

        let changed = record.change_endpoint(
            SshEndpoint::new("new.example.test", SSH_DEFAULT_PORT, "joe")
                .expect("changed endpoint"),
        );
        let reverified = changed
            .confirm(PinnedHostKey::new("ssh-ed25519", "AAAAC3NzaNew").expect("new host key"));
        assert_eq!(reverified.id(), id);
        assert_eq!(reverified.endpoint().host(), "new.example.test");
        assert_ne!(reverified.pinned_host_key(), &pin);
    }

    /// The agent-to-password fallback (P0-20) rides on this: the retry needs
    /// a password-shaped connect against the SAME confirmed server key. The
    /// flip must keep everything confirmation vouched for — id, endpoint,
    /// pinned key — and change the authentication alone, without a re-draft.
    #[test]
    fn flipping_to_password_authentication_keeps_the_confirmed_identity() {
        let id = ExecutionHostId::generate();
        let pin = PinnedHostKey::new("ssh-ed25519", "AAAAC3NzaAgent").expect("valid pin");
        let record = SshHostDraft::new(
            id,
            SshEndpoint::new("agent.example.test", SSH_DEFAULT_PORT, "joe").expect("endpoint"),
            SshAuthentication::Agent,
        )
        .confirm(pin.clone());

        let flipped = record.with_password_authentication();
        assert_eq!(flipped.authentication(), SshAuthentication::Password);
        assert_eq!(flipped.id(), id);
        assert_eq!(flipped.endpoint().host(), "agent.example.test");
        assert_eq!(flipped.pinned_host_key(), &pin);
    }

    #[test]
    fn passwords_never_enter_the_persisted_host_record() {
        let record = SshHostDraft::new(
            ExecutionHostId::generate(),
            SshEndpoint::new("dev.example.test", SSH_DEFAULT_PORT, "joe").expect("valid endpoint"),
            SshAuthentication::Password,
        )
        .confirm(PinnedHostKey::new("ssh-ed25519", "AAAAC3NzaPinned").expect("valid pin"));
        let wire = serde_json::to_value(record).expect("serialize host record");

        assert_eq!(wire["authentication"], "password");
        assert!(wire.get("password").is_none());
        assert!(wire.get("secret").is_none());
        assert!(wire.get("credential").is_none());
        let mut hostile = wire;
        hostile["password"] = serde_json::json!("must-not-survive");
        assert!(
            serde_json::from_value::<SshHostRecord>(hostile).is_err(),
            "an unknown plaintext credential was accepted by the host record"
        );
    }

    /// 오늘의 답은 하나다. 그리고 그 답을 내는 곳도 하나여야 한다.
    #[test]
    fn every_workspace_is_local_today() {
        assert_eq!(Host::for_workspace(Path::new("/tmp/anywhere")), Host::Local);
        assert_eq!(Host::for_workspace(Path::new("relative")), Host::Local);
    }

    /// 로컬 impl이 예전 자유 함수와 같은 답을 낸다 — 목록, 링크, 그리고 크기.
    #[test]
    fn the_local_listing_names_what_it_found_without_following_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("inside")).expect("mkdir");
        std::fs::write(dir.path().join("file"), b"1234567890").expect("write");

        let mut listed = LocalFs.read_dir(dir.path()).expect("read_dir");
        listed.sort_by_key(DirEntry::name);
        let names: Vec<String> = listed.iter().map(DirEntry::name).collect();
        assert_eq!(names, vec!["file".to_string(), "inside".to_string()]);

        let file = listed
            .iter()
            .find(|one| one.name() == "file")
            .expect("file");
        let about = file.about.expect("the file was described");
        assert!(!about.is_dir);
        // 점유 블록이라 논리 크기와 같지 않을 수 있다. 그러나 무언가는 차지한다.
        assert!(about.disk_bytes > 0);

        let inside = listed
            .iter()
            .find(|one| one.name() == "inside")
            .expect("dir");
        assert!(inside.about.expect("the dir was described").is_dir);
        assert!(LocalFs.is_dir(&inside.path));
        assert!(!LocalFs.is_dir(&file.path));
    }

    /// 링크는 링크로 보고된다 — 가리키는 것으로 접히지 않는다.
    #[cfg(unix)]
    #[test]
    fn a_link_to_a_directory_is_not_reported_as_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("real")).expect("mkdir");
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link"))
            .expect("symlink");

        let listed = LocalFs.read_dir(dir.path()).expect("read_dir");
        let link = listed
            .iter()
            .find(|one| one.name() == "link")
            .expect("the link was listed");
        assert!(
            !link.about.expect("described").is_dir,
            "the boundary followed a link, so a walk can leave the workspace"
        );
    }

    /// 열지 못한 이유 셋이 서로 다르게 돌아온다. 셋이 서로 다른 화면이다.
    #[test]
    fn a_directory_that_is_not_there_is_missing_not_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            LocalFs.read_dir(&dir.path().join("nope")),
            Err(ReadDir::Missing)
        );
    }

    /// 읽지 못한 시각은 0이 아니라 없음이다.
    #[test]
    fn an_unreadable_time_is_absent_not_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("touched");
        std::fs::write(&file, b"x").expect("write");
        assert!(LocalFs.modified_ms(&file).is_some_and(|at| at > 0));
        assert_eq!(LocalFs.modified_ms(&dir.path().join("nope")), None);
        assert_eq!(LocalFs.read_to_string(&dir.path().join("nope")), None);
        assert_eq!(LocalFs.read_to_string(&file).as_deref(), Some("x"));
    }

    /// 실패한 git은 자기가 한 말로 실패한다 — 빈 성공이 아니다.
    #[test]
    fn a_refused_git_read_carries_what_git_said() {
        let dir = tempfile::tempdir().expect("tempdir");
        let said = LocalVcs.text(dir.path(), &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert!(
            said.is_err(),
            "a directory that is not a repository answered a branch read"
        );
        assert!(
            !said.expect_err("err").is_empty(),
            "the failure carried no sentence, so nobody can read what went wrong"
        );
    }

    /// `text`와 `text_within`은 같은 물음에 같은 답을 한다.
    #[test]
    fn the_bounded_reader_agrees_with_the_plain_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = ["rev-parse", "--is-inside-work-tree"];
        let plain = LocalVcs.text(dir.path(), &args).ok();
        let bounded = LocalVcs.text_within(dir.path(), &args, Duration::from_secs(8));
        assert_eq!(plain, bounded);
    }

    /// [`None`]은 "답을 못 받았다"이고, 답이 빈 것과 다르다.
    ///
    /// 시간 초과로 죽이는 쪽은 시계를 이기는 시험이라 여기서 재현하지 않는다 —
    /// 그 불변식(파이프를 자기 스레드가 비우고, 포기한 자식을 죽인다)은 창의
    /// 게이트가 이 파일의 소스에 대고 붙잡는다.
    #[test]
    fn an_unanswered_read_is_absent_and_an_answered_one_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let budget = Duration::from_secs(8);
        assert!(
            LocalVcs
                .text_within(dir.path(), &["--version"], budget)
                .is_some_and(|said| said.starts_with("git version")),
            "a read that git answered came back as no answer"
        );
        assert_eq!(
            LocalVcs.text_within(
                dir.path(),
                &["rev-parse", "--abbrev-ref", "@{upstream}"],
                budget
            ),
            None,
            "a refused read came back as an answer"
        );
    }

    /// 거절에는 **이유가 실려 있다** — 접힌 얼굴이 잃는 바로 그것.
    ///
    /// 이 구별이 있어야 부른 쪽이 "없는 브랜치"와 "만료된 토큰"과 "끊긴 회선"을
    /// 다르게 대접할 수 있다. 셋 다 git에게는 0이 아닌 종료값 하나다.
    #[test]
    fn a_refusal_carries_the_sentence_the_folded_face_throws_away() {
        let dir = tempfile::tempdir().expect("tempdir");
        let budget = Duration::from_secs(8);
        let refused = LocalVcs.try_within(dir.path(), &["rev-parse", "--verify", "nope"], budget);
        let Within::Refused(said) = &refused else {
            panic!("a failed read did not come back as a refusal: {refused:?}");
        };
        assert!(
            !said.is_empty(),
            "the refusal carried no sentence, so nobody can read what went wrong"
        );
        assert!(
            matches!(
                LocalVcs.try_within(dir.path(), &["--version"], budget),
                Within::Said(said) if said.starts_with("git version")
            ),
            "a read git answered did not come back as an answer"
        );
        // 그리고 접는 얼굴은 셋을 접는다 — 하나가 다른 하나를 흉내 내지 않는다.
        assert_eq!(
            LocalVcs.text_within(dir.path(), &["rev-parse", "--verify", "nope"], budget),
            None
        );
    }
}
