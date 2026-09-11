// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) const UNDO_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;

pub(super) const UNDO_LIMIT_PER_SITE: i64 = 10;

pub(super) const IDEMPOTENCY_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;

pub(super) const SQLITE_DELETE_BATCH_SIZE: usize = 500;

pub(super) const MANIFEST_PATH: &str = "symbol.toml";

pub(super) const RESERVED_TERMINALS: [&str; 7] = [
    "FILES",
    "HASH",
    "UNDO",
    "EXPIRES",
    "symbol.toml",
    ".symbol-token",
    ".symbol-claim",
];

pub(super) const DEFAULT_BLOB_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(super) const DEFAULT_BLOB_CACHE_ENTRIES: usize = 16 * 1024;

pub(super) const BLOB_CACHE_ENTRY_OVERHEAD: usize = 128;

pub(super) const MAX_READ_CONNECTIONS: usize = 8;

pub(super) const PENDING_RETENTION_MILLIS: i64 = 15 * 60 * 1000;

#[cfg(test)]
pub(super) const MAX_SPLICE_RESULT_SIZE: u64 = 4 * 1024 * 1024 * 1024;

pub(super) const MAX_ALIAS_HOPS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[repr(i64)]
pub enum AllocatedNamingMode {
    ContentAddressed = 1,
    Custom = 2,
}

impl TryFrom<i64> for AllocatedNamingMode {
    type Error = StoreError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::ContentAddressed),
            2 => Ok(Self::Custom),
            _ => Err(StoreError::InvalidAllocatedName),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone)]
pub struct DirEnt {
    pub kind: EntryKind,
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct DirList {
    pub files: u64,
    pub bytes: u64,
    pub alias_count: u64,
    pub aliases: Vec<AliasEntry>,
    pub entries: Vec<DirEnt>,
}

#[derive(Debug, Clone)]
pub struct SiteEnt {
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct SiteList {
    pub files: u64,
    pub alias_count: u64,
    pub bytes: u64,
    pub entries: Vec<SiteEnt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ArchiveFormat {
    Tar,
    TarGz,
    Zip,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct UndoInfo {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MutationResult {
    pub created: bool,
    pub changed: bool,
    #[serde(default)]
    pub replayed: bool,
    pub files: usize,
    pub revision: u64,
    pub tree_hash: String,
    pub undo: Option<UndoInfo>,
    pub sanitized: TokenCounts,
}

#[derive(Debug, Clone)]
pub struct ExpiryMutation {
    pub report: ExpiryReport,
    pub undo: Option<UndoInfo>,
}

#[derive(Debug, Clone)]
pub struct Idempotency {
    pub key: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AllocatedName<'a> {
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub extension: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct AllocationSpec<'a> {
    pub folder: &'a str,
    pub naming: AllocatedName<'a>,
    pub media_type: &'a str,
}

impl Default for AllocationSpec<'_> {
    fn default() -> Self {
        Self {
            folder: "",
            naming: AllocatedName::default(),
            media_type: "application/octet-stream",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PendingAllocationSpec<'a> {
    pub folder: &'a str,
    pub media_type: &'a str,
    pub extension: Option<&'a str>,
}

impl Default for PendingAllocationSpec<'_> {
    fn default() -> Self {
        Self {
            folder: "",
            media_type: "application/octet-stream",
            extension: None,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AllocatedFile {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub changed: bool,
    #[serde(default)]
    pub replayed: bool,
    pub mutation: Option<MutationResult>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PendingAllocation {
    pub token: String,
    pub folder: String,
    pub hash: String,
    pub size: u64,
    pub media_type: String,
    #[serde(default)]
    pub extension: Option<String>,
    pub expires_at: String,
    #[serde(default)]
    pub tree_hash: String,
    #[serde(default)]
    pub content_revision: u64,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AllocationCancellation {
    pub replayed: bool,
    pub tree_hash: String,
    pub content_revision: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileExpiry {
    #[default]
    Preserve,
    Clear,
    Policy(ExpiryPolicy),
}

#[derive(Debug, Clone, Copy)]
pub enum AllocationSource<'a> {
    #[cfg(test)]
    Bytes(&'a [u8]),
    File(&'a Path),
}

#[derive(Debug, Clone, Copy)]
pub enum SpliceSource<'a> {
    Empty,
    #[cfg(test)]
    Bytes(&'a [u8]),
    File(&'a Path),
}

#[derive(Debug, Clone, Copy)]
pub struct Splice<'a> {
    pub offset: u64,
    pub delete: u64,
    pub insert: SpliceSource<'a>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FileMutationOptions<'a> {
    pub expected_tree_hash: Option<&'a str>,
    pub idempotency: Option<&'a Idempotency>,
    pub authorization: Option<&'a ManagementToken>,
    pub expiry: FileExpiry,
}

#[derive(Debug, Clone, Copy)]
pub struct AliasSpec<'a> {
    pub path: &'a str,
    pub target: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[repr(i64)]
pub enum AliasResolvedKind {
    File = 0,
    Directory = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct AliasEntry {
    pub path: String,
    pub canonical_target: String,
    pub resolved_kind: Option<AliasResolvedKind>,
    pub resolved_hash: Option<String>,
    pub resolved_size: Option<u64>,
    pub resolved_files: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AliasMutationResult {
    pub mutation: MutationResult,
    pub aliases: Vec<AliasEntry>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasInventory {
    pub site: String,
    pub content_revision: u64,
    pub tree_hash: String,
    pub aliases: Vec<AliasEntry>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AliasStats {
    pub aliases: u64,
    pub resolved: u64,
    pub dangling: u64,
}

#[derive(Clone, Copy, Default)]
pub struct PublishOptions<'a> {
    pub expected_tree_hash: Option<&'a str>,
    pub idempotency: Option<&'a Idempotency>,
    pub creation: CreationSecurity,
    pub authorization: Option<&'a ManagementToken>,
    pub replace: bool,
}

#[derive(Clone, Copy, Default)]
pub struct ManagementRequest<'a> {
    pub idempotency: Option<&'a Idempotency>,
    pub audit_ip: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum CreatorKind {
    TrustedProxy = 1,
    Mtls = 2,
    Tailscale = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatorIdentity {
    pub(super) kind: CreatorKind,
    pub(super) hash: [u8; 32],
}

impl CreatorIdentity {
    #[must_use]
    pub fn trusted_proxy(principal: &str) -> Self {
        Self {
            kind: CreatorKind::TrustedProxy,
            hash: blake3::derive_key("symbol trusted proxy principal v1", principal.as_bytes()),
        }
    }

    #[must_use]
    pub fn mtls(fingerprint: &str) -> Self {
        Self {
            kind: CreatorKind::Mtls,
            hash: blake3::derive_key("symbol mTLS creator principal v1", fingerprint.as_bytes()),
        }
    }

    #[must_use]
    pub fn tailscale(user: &str) -> Self {
        Self {
            kind: CreatorKind::Tailscale,
            hash: blake3::derive_key("symbol Tailscale creator principal v1", user.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CreationSecurity {
    pub creator: Option<CreatorIdentity>,
    pub claim_hash: Option<ClaimTokenHash>,
    pub management_hash: Option<ManagementTokenHash>,
}

#[derive(Debug)]
pub struct ManagementMutation {
    pub status: ManagementStatus,
    pub token: Option<ManagementToken>,
    pub replayed: bool,
}

#[derive(Debug, Clone)]
pub struct UndoResult {
    pub restored_at: String,
}

#[derive(Debug)]
pub struct PopResult {
    pub size: u64,
    pub undo: UndoInfo,
}

#[derive(Debug)]
pub enum Node {
    Dir,
    File { logical: String, hash: ContentHash },
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Name(#[from] NameError),
    #[error("{0}")]
    Path(#[from] PathError),
    #[error("{0}")]
    Upload(#[from] UploadError),
    #[error("error: site not found")]
    NotFound,
    #[error("error: undo token is stale; latest token is {0}")]
    StaleUndo(Box<str>),
    #[error("error: unsupported undo kind {0}")]
    UnsupportedUndoKind(i64),
    #[error("error: destination site already exists")]
    DestinationConflict,
    #[error("error: alias conflicts with an existing entry")]
    AliasConflict,
    #[error("error: writes through aliases are not allowed")]
    AliasWrite,
    #[error("error: alias target is invalid")]
    InvalidAliasTarget,
    #[error("error: alias cycle detected")]
    AliasCycle,
    #[error("error: alias resolution exceeded {MAX_ALIAS_HOPS} hops")]
    AliasHopLimit,
    #[error("error: idempotency key was already used for a different request")]
    IdempotencyConflict,
    #[error("error: idempotency key must be 1-256 visible ASCII characters")]
    InvalidIdempotencyKey,
    #[error("error: upstream changed; nothing was written")]
    PreconditionFailed { revision: u64, tree_hash: TreeHash },
    #[error("error: file content hash is stale; current hash is {}", _0.to_wire())]
    StaleContentHash(ContentHash),
    #[error("{0}")]
    HashParse(#[from] HashParseError),
    #[error("error: invalid allocated file name")]
    InvalidAllocatedName,
    #[error("error: pending allocation token is invalid, expired, or already consumed")]
    InvalidPendingAllocation,
    #[error("error: splice ranges must be ordered and non-overlapping")]
    InvalidSpliceOrder,
    #[error("error: splice range is outside the original file")]
    SpliceRange,
    #[error("error: splice result exceeds the configured limit")]
    SpliceResultTooLarge,
    #[error("error: management token required")]
    Unauthorized,
    #[error("error: creator identity or claim is not authorized")]
    Forbidden,
    #[error("error: site is already managed")]
    AlreadyManaged,
    #[error("error: {0}")]
    Expiry(#[from] ExpiryError),
    #[error("error: sqlite: {0}")]
    Sqlite(#[from] diesel::result::Error),
    #[error("error: sqlite connection: {0}")]
    Connection(#[from] diesel::ConnectionError),
    #[error("error: database migration: {0}")]
    Migration(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("error: operating system random source failed")]
    Random(#[from] getrandom::Error),
    #[error("error: {0}")]
    Io(#[from] io::Error),
    #[error("error: startup phase {phase}: {source}")]
    Startup {
        phase: &'static str,
        source: Box<Self>,
    },
}

impl StoreError {
    pub(super) fn startup(phase: &'static str, source: Self) -> Self {
        Self::Startup {
            phase,
            source: Box::new(source),
        }
    }
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = sites)]
pub(super) struct ManagementSiteRow {
    pub(super) id: i64,
    pub(super) creator_kind: Option<i64>,
    pub(super) creator_hash: Option<Vec<u8>>,
    pub(super) claim_hash: Option<Vec<u8>>,
    pub(super) management_hash: Option<Vec<u8>>,
    pub(super) management_status: i64,
}

#[derive(Insertable)]
#[diesel(table_name = sites)]
pub(super) struct NewSite<'a> {
    pub(super) name: &'a str,
    pub(super) created: Option<i64>,
    pub(super) updated: i64,
    pub(super) public_url: &'a str,
    pub(super) content_revision: i64,
    pub(super) tree_hash: TreeHash,
    pub(super) creator_kind: Option<i64>,
    pub(super) creator_hash: Option<Vec<u8>>,
    pub(super) claim_hash: Option<Vec<u8>>,
    pub(super) management_hash: Option<Vec<u8>>,
    pub(super) management_status: i64,
}

#[derive(Queryable, Selectable, Insertable)]
#[diesel(table_name = files)]
pub(super) struct FileRow {
    pub(super) site_id: i64,
    pub(super) path: String,
    pub(super) kind: i64,
    pub(super) hash: ContentHash,
    pub(super) size: i64,
}

#[derive(Insertable)]
#[diesel(table_name = files)]
pub(super) struct NewFile {
    pub(super) site_id: i64,
    pub(super) path: String,
    pub(super) hash: ContentHash,
    pub(super) size: i64,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = aliases)]
pub(super) struct AliasRow {
    pub(super) path: String,
    pub(super) canonical_target: String,
    pub(super) resolved_kind: Option<i64>,
    pub(super) resolved_hash: Option<ContentHash>,
    pub(super) resolved_size: Option<i64>,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = undo_alias_deltas)]
pub(super) struct UndoAliasRow {
    pub(super) path: String,
    pub(super) existed: i64,
    pub(super) canonical_target: Option<String>,
    pub(super) resolved_kind: Option<i64>,
    pub(super) resolved_hash: Option<ContentHash>,
    pub(super) resolved_size: Option<i64>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = expiry_policies)]
pub(super) struct ExpiryPolicyRow {
    pub(super) path: String,
    pub(super) target_kind: i64,
    pub(super) mode: i64,
    pub(super) duration_seconds: Option<i64>,
    pub(super) deadline: Option<i64>,
    pub(super) min_age_seconds: Option<i64>,
    pub(super) max_age_seconds: Option<i64>,
    pub(super) max_size_bytes: Option<i64>,
    pub(super) power: Option<f64>,
    pub(super) refreshed: Option<i64>,
    pub(super) own_deadline: Option<i64>,
    pub(super) size_bytes: i64,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = undo_expiry_policies)]
pub(super) struct UndoExpiryPolicyRow {
    pub(super) path: String,
    pub(super) target_kind: i64,
    pub(super) mode: i64,
    pub(super) duration_seconds: Option<i64>,
    pub(super) deadline: Option<i64>,
    pub(super) min_age_seconds: Option<i64>,
    pub(super) max_age_seconds: Option<i64>,
    pub(super) max_size_bytes: Option<i64>,
    pub(super) power: Option<f64>,
    pub(super) refreshed: Option<i64>,
    pub(super) own_deadline: i64,
    pub(super) size_bytes: i64,
}

#[derive(QueryableByName)]
pub(super) struct IntegrityCheck {
    #[diesel(sql_type = Text)]
    pub(super) integrity_check: String,
}

#[derive(QueryableByName)]
pub(super) struct ForeignKeyViolationCount {
    #[diesel(sql_type = BigInt)]
    pub(super) violation_count: i64,
}

#[cfg(test)]
#[derive(QueryableByName)]
pub(super) struct AliasUpdateCount {
    #[diesel(sql_type = BigInt)]
    pub(super) count: i64,
}
