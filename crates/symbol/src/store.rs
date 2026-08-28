#![cfg_attr(not(test), allow(dead_code))]

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use diesel::connection::{AnsiTransactionManager, SimpleConnection, TransactionManager};
use diesel::dsl::{count_star, min};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use diesel::upsert::excluded;
use flate2::Compression;
use flate2::write::GzEncoder;
pub use symbol_contract::{
    AliasTargetKind, CacheStats, InventoryAlias, InventoryFile, ManagementStatus, ReaderStats,
    ServingStats, SiteInventory, SizeDistribution, Stats, UndoEntry, UndoStack,
};

use crate::blob_store::BlobFiles;
use crate::database;
use crate::expiry::{
    DecayPolicy, ExpiryError, ExpiryLimit, ExpiryMode, ExpiryPolicy, ExpiryReport,
    ExpirySiteReport, ExpiryTarget, ExpiryTargetKind, InheritedExpiryCap, OwnExpiryReport,
    remaining_seconds,
};
use crate::name::{NameError, generate_id, parse_site_name};
use crate::pathutil::{PathError, is_junk, is_noise_path, safe_rel_path};
use crate::sanitize::{self, TokenCounts};
use crate::schema::{
    aliases, allocated_entries, blobs, expiry_policies, files, idempotency_records,
    management_audit, management_idempotency, management_tombstones, metadata, path_aggregates,
    pending_allocations, site_entries, sites, undo_alias_deltas, undo_allocated_deltas,
    undo_expiry_policies, undo_file_deltas, undo_files, undo_names, undo_operations, undo_sites,
};
use crate::secrets::{ClaimToken, ClaimTokenHash, ManagementToken, ManagementTokenHash};
#[cfg(test)]
use crate::upload::write_payload;
use crate::upload::{
    ArchiveMember, ArchivePlan, Kind, MAX_ALIAS_TARGET_BYTES, UploadError, plan_archive,
    write_payload_file,
};

#[cfg(test)]
use std::io::Cursor;

const UNDO_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;
const UNDO_LIMIT_PER_SITE: i64 = 10;
const IDEMPOTENCY_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;
const SQLITE_DELETE_BATCH_SIZE: usize = 500;
const MANIFEST_PATH: &str = "symbol.toml";
const RESERVED_TERMINALS: [&str; 7] = [
    "FILES",
    "HASH",
    "UNDO",
    "EXPIRES",
    "symbol.toml",
    ".symbol-token",
    ".symbol-claim",
];
const DEFAULT_BLOB_CACHE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_BLOB_CACHE_ENTRIES: usize = 16 * 1024;
const BLOB_CACHE_ENTRY_OVERHEAD: usize = 128;
const MAX_READ_CONNECTIONS: usize = 8;
const PENDING_RETENTION_MILLIS: i64 = 15 * 60 * 1000;
const MAX_SPLICE_RESULT_SIZE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ALIAS_HOPS: usize = 64;

#[cfg(test)]
type ContentCommitHook = Box<dyn FnOnce() + Send>;

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    writer: Mutex<SqliteConnection>,
    readers: ReaderPool,
    blobs: BlobCache,
    blob_files: BlobFiles,
    metrics: Arc<Metrics>,
    temp_generation: AtomicU64,
    public_url: String,
    clock: Arc<dyn Clock>,
    expiry_defaults: DecayPolicy,
    #[cfg(test)]
    before_content_commit: Mutex<Option<ContentCommitHook>>,
}

trait Clock: Send + Sync {
    fn now_millis(&self) -> i64;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        system_now_millis()
    }
}

struct ReaderPool {
    available: Mutex<Vec<SqliteConnection>>,
    ready: Condvar,
    size: usize,
    metrics: Arc<Metrics>,
}

struct Reader<'a> {
    pool: &'a ReaderPool,
    connection: Option<SqliteConnection>,
    acquired: Instant,
}

struct DbTransaction<'a> {
    connection: &'a mut SqliteConnection,
    finished: bool,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = sites)]
struct ManagementSiteRow {
    id: i64,
    creator_kind: Option<i64>,
    creator_hash: Option<Vec<u8>>,
    claim_hash: Option<Vec<u8>>,
    management_hash: Option<Vec<u8>>,
    management_status: i64,
}

#[derive(Insertable)]
#[diesel(table_name = sites)]
struct NewSite<'a> {
    name: &'a str,
    updated: i64,
    public_url: &'a str,
    content_revision: i64,
    tree_hash: &'a str,
    creator_kind: Option<i64>,
    creator_hash: Option<Vec<u8>>,
    claim_hash: Option<Vec<u8>>,
    management_hash: Option<Vec<u8>>,
    management_status: i64,
}

#[derive(Queryable, Selectable, Insertable)]
#[diesel(table_name = files)]
struct FileRow {
    site_id: i64,
    path: String,
    kind: i64,
    hash: String,
    size: i64,
}

#[derive(Insertable)]
#[diesel(table_name = files)]
struct NewFile<'a> {
    site_id: i64,
    path: &'a str,
    hash: &'a str,
    size: i64,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = aliases)]
struct AliasRow {
    path: String,
    canonical_target: String,
    resolved_kind: Option<i64>,
    resolved_hash: Option<String>,
    resolved_size: Option<i64>,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = undo_alias_deltas)]
struct UndoAliasRow {
    path: String,
    existed: i64,
    canonical_target: Option<String>,
    resolved_kind: Option<i64>,
    resolved_hash: Option<String>,
    resolved_size: Option<i64>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllocatedMetadata {
    hash: String,
    size: i64,
    naming_mode: AllocatedNamingMode,
    prefix: String,
    suffix: String,
    extension: Option<String>,
    media_type: String,
}

#[derive(Debug, Clone)]
struct PendingMetadata {
    folder: String,
    hash: String,
    size: i64,
    media_type: String,
    request_fingerprint: String,
    expiry: FileExpiry,
    authorization_hash: Option<String>,
    extension: Option<String>,
    expected_tree_hash: Option<String>,
    legacy_fingerprint: bool,
    sanitized: TokenCounts,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PendingRequestMetadata {
    request_fingerprint: String,
    expiry: FileExpiry,
    #[serde(default)]
    authorization_hash: Option<String>,
    #[serde(default)]
    extension: Option<String>,
    #[serde(default)]
    expected_tree_hash: Option<String>,
    #[serde(default)]
    legacy_fingerprint: bool,
    #[serde(default)]
    sanitized: TokenCounts,
}

#[derive(Clone, Copy)]
struct PendingFingerprint<'a> {
    site: &'a str,
    folder: &'a str,
    hash: &'a str,
    content_size: u64,
    media_type: &'a str,
    expiry: FileExpiry,
    authorization_hash: Option<&'a str>,
    extension: Option<&'a str>,
    expected_tree_hash: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct PendingFinalizeFingerprint<'a> {
    site: &'a str,
    token: &'a str,
    folder: Option<&'a str>,
    final_name: PendingFinalName<'a>,
    expiry: FileExpiry,
    authorization_hash: Option<&'a str>,
    expected_tree_hash: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct AllocatedEntryFingerprint<'a> {
    name: &'a str,
    current_path: Option<&'a str>,
    destination: &'a AllocationDestination,
    hash: &'a str,
    kind: UndoKind,
    expiry: FileExpiry,
    expected_tree_hash: Option<&'a str>,
}

struct AllocationDestination {
    path: String,
    naming_mode: AllocatedNamingMode,
    prefix: String,
    suffix: String,
    extension: Option<String>,
    media_type: String,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = undo_allocated_deltas)]
struct UndoAllocatedMetadata {
    path: String,
    existed: i64,
    hash: Option<String>,
    size: Option<i64>,
    naming_mode: Option<i64>,
    prefix: Option<String>,
    suffix: Option<String>,
    extension: Option<String>,
    media_type: Option<String>,
}

#[derive(Clone, Copy)]
enum PendingFinalName<'a> {
    Generated(AllocatedName<'a>),
    Custom(&'a str),
}

fn ensure_file_entry(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(site_entries::table)
        .values((
            site_entries::site_id.eq(site_id),
            site_entries::path.eq(path),
            site_entries::kind.eq(database::schema::FILE_ENTRY_KIND),
        ))
        .on_conflict_do_nothing()
        .execute(db)?;
    Ok(())
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = expiry_policies)]
struct ExpiryPolicyRow {
    path: String,
    target_kind: i64,
    mode: i64,
    duration_seconds: Option<i64>,
    deadline: Option<i64>,
    min_age_seconds: Option<i64>,
    max_age_seconds: Option<i64>,
    max_size_bytes: Option<i64>,
    power: Option<f64>,
    refreshed: Option<i64>,
    own_deadline: Option<i64>,
    size_bytes: i64,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = undo_expiry_policies)]
struct UndoExpiryPolicyRow {
    path: String,
    target_kind: i64,
    mode: i64,
    duration_seconds: Option<i64>,
    deadline: Option<i64>,
    min_age_seconds: Option<i64>,
    max_age_seconds: Option<i64>,
    max_size_bytes: Option<i64>,
    power: Option<f64>,
    refreshed: Option<i64>,
    own_deadline: i64,
    size_bytes: i64,
}

#[derive(QueryableByName)]
struct IntegrityCheck {
    #[diesel(sql_type = Text)]
    integrity_check: String,
}

#[derive(QueryableByName)]
struct ForeignKeyViolationCount {
    #[diesel(sql_type = BigInt)]
    violation_count: i64,
}

#[cfg(test)]
#[derive(QueryableByName)]
struct AliasUpdateCount {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct AliasDirectoryRowWork {
    resolution: u64,
    files: u64,
    allocated: u64,
    aliases: u64,
    aggregates: u64,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct AliasRefreshRowWork {
    real: u64,
    aliases: u64,
}

#[cfg(test)]
thread_local! {
    static ALIAS_DIRECTORY_ROW_WORK: Cell<AliasDirectoryRowWork> =
        Cell::new(AliasDirectoryRowWork::default());
    static ALIAS_REFRESH_ROW_WORK: Cell<AliasRefreshRowWork> =
        Cell::new(AliasRefreshRowWork::default());
}

#[cfg(test)]
fn reset_alias_directory_row_work() {
    ALIAS_DIRECTORY_ROW_WORK.set(AliasDirectoryRowWork::default());
}

#[cfg(test)]
fn alias_directory_row_work() -> AliasDirectoryRowWork {
    ALIAS_DIRECTORY_ROW_WORK.get()
}

#[cfg(test)]
fn reset_alias_refresh_row_work() {
    ALIAS_REFRESH_ROW_WORK.set(AliasRefreshRowWork::default());
}

#[cfg(test)]
fn alias_refresh_row_work() -> AliasRefreshRowWork {
    ALIAS_REFRESH_ROW_WORK.get()
}

#[cfg(test)]
fn record_alias_refresh_rows(real: usize, aliases: usize) {
    ALIAS_REFRESH_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.real += u64::try_from(real).expect("row count fits in u64");
        work.aliases += u64::try_from(aliases).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_refresh_rows(_: usize, _: usize) {}

#[cfg(test)]
fn record_alias_resolution_rows(rows: usize) {
    ALIAS_DIRECTORY_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.resolution += u64::try_from(rows).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_resolution_rows(_: usize) {}

#[cfg(test)]
fn record_alias_listed_file_rows(rows: usize) {
    ALIAS_DIRECTORY_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.files += u64::try_from(rows).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_listed_file_rows(_: usize) {}

#[cfg(test)]
fn record_alias_listed_allocated_rows(rows: usize) {
    ALIAS_DIRECTORY_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.allocated += u64::try_from(rows).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_listed_allocated_rows(_: usize) {}

#[cfg(test)]
fn record_alias_listed_rows(rows: usize) {
    ALIAS_DIRECTORY_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.aliases += u64::try_from(rows).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_listed_rows(_: usize) {}

#[cfg(test)]
fn record_alias_aggregate_rows(rows: usize) {
    ALIAS_DIRECTORY_ROW_WORK.with(|cell| {
        let mut work = cell.get();
        work.aggregates += u64::try_from(rows).expect("row count fits in u64");
        cell.set(work);
    });
}

#[cfg(not(test))]
const fn record_alias_aggregate_rows(_: usize) {}

struct BlobCache {
    capacity: usize,
    max_entries: usize,
    state: Mutex<BlobCacheState>,
    metrics: Arc<Metrics>,
}

struct BlobCacheState {
    entries: HashMap<String, CachedBlob>,
    recency: BTreeMap<u64, String>,
    charge: usize,
    generation: u64,
}

struct CachedBlob {
    bytes: Bytes,
    last_used: u64,
    charge: usize,
}

#[derive(Default)]
struct Metrics {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_evictions: AtomicU64,
    reader_operations: AtomicU64,
    reader_waits: AtomicU64,
    reader_wait_micros: AtomicU64,
    reader_query_micros: AtomicU64,
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
    Bytes(&'a [u8]),
    File(&'a Path),
}

#[derive(Debug, Clone, Copy)]
pub enum SpliceSource<'a> {
    Empty,
    Bytes(&'a [u8]),
    File(&'a Path),
}

#[derive(Debug, Clone, Copy)]
pub struct Splice<'a> {
    pub offset: u64,
    pub delete: u64,
    pub insert: SpliceSource<'a>,
}

enum PreparedSpliceSource {
    Empty,
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        size: u64,
        hash: String,
    },
}

struct PreparedSplice {
    offset: u64,
    delete: u64,
    insert: PreparedSpliceSource,
    sanitized: TokenCounts,
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn create(path: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn persist(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasInventory {
    pub site: String,
    pub content_revision: u64,
    pub tree_hash: String,
    pub aliases: Vec<AliasEntry>,
}

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
    kind: CreatorKind,
    hash: [u8; 32],
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
    File { logical: String, hash: String },
}

struct StagedFile {
    path: String,
    size: i64,
    hash: String,
    source: StagedSource,
    sanitized: TokenCounts,
}

enum StagedSource {
    Bytes(Vec<u8>),
    File(PathBuf),
    Temporary(PathBuf),
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let StagedSource::Temporary(path) = &self.source
            && let Some(parent) = path.parent()
        {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

#[cfg(test)]
enum ArchiveFile {
    File { path: String, bytes: Vec<u8> },
    Alias { path: String, target: String },
}

enum ArchiveEntry {
    File {
        path: String,
        hash: String,
        size: u64,
    },
    Alias {
        path: String,
        target: String,
    },
}

#[cfg(test)]
struct SiteArchive {
    files: Vec<ArchiveFile>,
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
    StaleUndo(String),
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
    PreconditionFailed { revision: u64, tree_hash: String },
    #[error("error: file content hash is stale; current hash is {0}")]
    StaleContentHash(String),
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
    #[error("error: reserved path already exists: {0}")]
    #[allow(dead_code)]
    ReservedCollision(String),
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
    fn startup(phase: &'static str, source: Self) -> Self {
        Self::Startup {
            phase,
            source: Box::new(source),
        }
    }
}

impl ReaderPool {
    fn open(path: &Path, count: usize, metrics: Arc<Metrics>) -> Result<Self, StoreError> {
        let mut available = Vec::with_capacity(count);
        for _ in 0..count {
            let database_url = format!("file:{}?mode=ro", path.display());
            let mut connection = SqliteConnection::establish(&database_url)?;
            connection.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")?;
            available.push(connection);
        }
        Ok(Self {
            available: Mutex::new(available),
            ready: Condvar::new(),
            size: count,
            metrics,
        })
    }

    fn get(&self) -> Reader<'_> {
        let wait_started = Instant::now();
        let mut available = self.available.lock().unwrap();
        let waited = available.is_empty();
        while available.is_empty() {
            available = self.ready.wait(available).unwrap();
        }
        if waited {
            let micros = elapsed_micros(wait_started);
            self.metrics.reader_waits.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .reader_wait_micros
                .fetch_add(micros, Ordering::Relaxed);
            tracing::debug!(wait_micros = micros, "waited for SQLite reader");
        }
        Reader {
            pool: self,
            connection: available.pop(),
            acquired: Instant::now(),
        }
    }

    const fn size(&self) -> usize {
        self.size
    }
}

impl std::ops::Deref for Reader<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for Reader<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().unwrap()
    }
}

impl DbTransaction<'_> {
    fn begin(
        connection: &mut SqliteConnection,
    ) -> Result<DbTransaction<'_>, diesel::result::Error> {
        AnsiTransactionManager::begin_transaction(connection)?;
        Ok(DbTransaction {
            connection,
            finished: false,
        })
    }

    fn commit(mut self) -> Result<(), diesel::result::Error> {
        AnsiTransactionManager::commit_transaction(self.connection)?;
        self.finished = true;
        Ok(())
    }
}

impl std::ops::Deref for DbTransaction<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        self.connection
    }
}

impl std::ops::DerefMut for DbTransaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
    }
}

impl Drop for DbTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = AnsiTransactionManager::rollback_transaction(self.connection);
        }
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        self.pool
            .metrics
            .reader_operations
            .fetch_add(1, Ordering::Relaxed);
        self.pool
            .metrics
            .reader_query_micros
            .fetch_add(elapsed_micros(self.acquired), Ordering::Relaxed);
        let connection = self.connection.take().unwrap();
        self.pool.available.lock().unwrap().push(connection);
        self.pool.ready.notify_one();
    }
}

impl BlobCache {
    fn new(capacity: usize, max_entries: usize, metrics: Arc<Metrics>) -> Self {
        Self {
            capacity,
            max_entries,
            state: Mutex::new(BlobCacheState {
                entries: HashMap::new(),
                recency: BTreeMap::new(),
                charge: 0,
                generation: 0,
            }),
            metrics,
        }
    }

    fn get(&self, hash: &str) -> Option<Bytes> {
        let mut state = self.state.lock().unwrap();
        let Some((last_used, bytes)) = state
            .entries
            .get(hash)
            .map(|entry| (entry.last_used, entry.bytes.clone()))
        else {
            drop(state);
            self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let key = state
            .recency
            .remove(&last_used)
            .expect("cached blob has recency entry");
        state.generation += 1;
        let generation = state.generation;
        state.recency.insert(generation, key);
        state
            .entries
            .get_mut(hash)
            .expect("cached blob still exists")
            .last_used = generation;
        drop(state);
        self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
        Some(bytes)
    }

    fn insert(&self, hash: &str, bytes: Bytes) {
        let charge = bytes
            .len()
            .saturating_add(hash.len().saturating_mul(2))
            .saturating_add(BLOB_CACHE_ENTRY_OVERHEAD);
        if charge > self.capacity || self.max_entries == 0 {
            return;
        }

        let mut state = self.state.lock().unwrap();
        if let Some(previous) = state.entries.remove(hash) {
            state.recency.remove(&previous.last_used);
            state.charge -= previous.charge;
        }
        let mut evictions = 0;
        while state.charge + charge > self.capacity || state.entries.len() >= self.max_entries {
            let Some((_, oldest)) = state.recency.pop_first() else {
                break;
            };
            let removed = state.entries.remove(&oldest).unwrap();
            state.charge -= removed.charge;
            evictions += 1;
        }
        state.generation += 1;
        let generation = state.generation;
        let hash = hash.to_string();
        state.recency.insert(generation, hash.clone());
        state.entries.insert(
            hash,
            CachedBlob {
                bytes,
                last_used: generation,
                charge,
            },
        );
        state.charge += charge;
        drop(state);
        if evictions > 0 {
            self.metrics
                .cache_evictions
                .fetch_add(evictions, Ordering::Relaxed);
            tracing::debug!(evictions, "evicted cached blobs");
        }
    }

    fn remove(&self, hashes: &[String]) {
        let mut state = self.state.lock().unwrap();
        for hash in hashes {
            if let Some(removed) = state.entries.remove(hash) {
                state.recency.remove(&removed.last_used);
                state.charge -= removed.charge;
            }
        }
    }

    #[cfg(test)]
    fn contains(&self, hash: &str) -> bool {
        self.state.lock().unwrap().entries.contains_key(hash)
    }
}

impl Metrics {
    fn snapshot(&self) -> ServingStats {
        ServingStats {
            cache: CacheStats {
                hits: self.cache_hits.load(Ordering::Relaxed),
                misses: self.cache_misses.load(Ordering::Relaxed),
                evictions: self.cache_evictions.load(Ordering::Relaxed),
            },
            readers: ReaderStats {
                operations: self.reader_operations.load(Ordering::Relaxed),
                waits: self.reader_waits.load(Ordering::Relaxed),
                wait_micros: self.reader_wait_micros.load(Ordering::Relaxed),
                query_micros: self.reader_query_micros.load(Ordering::Relaxed),
            },
        }
    }
}

impl Store {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Result<Self, StoreError> {
        Self::with_options(
            root,
            "http://symbol".to_string(),
            Arc::new(SystemClock),
            DecayPolicy::default(),
        )
    }

    #[cfg(test)]
    pub fn with_public_url(root: PathBuf, public_url: String) -> Result<Self, StoreError> {
        Self::with_expiry_defaults(root, public_url, DecayPolicy::default())
    }

    pub fn with_expiry_defaults(
        root: PathBuf,
        public_url: String,
        expiry_defaults: DecayPolicy,
    ) -> Result<Self, StoreError> {
        Self::with_options(root, public_url, Arc::new(SystemClock), expiry_defaults)
    }

    fn with_options(
        root: PathBuf,
        public_url: String,
        clock: Arc<dyn Clock>,
        expiry_defaults: DecayPolicy,
    ) -> Result<Self, StoreError> {
        let expiry_defaults = expiry_defaults.validate()?;
        fs::create_dir_all(&root)?;
        let tmp = root.join("tmp");
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        let path = root.join("symbol.db");
        let mut db = SqliteConnection::establish(&path.to_string_lossy())?;
        db.batch_execute(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        run_migrations(&mut db)?;
        let reader_count = std::thread::available_parallelism()
            .map_or(4, std::num::NonZeroUsize::get)
            .clamp(2, MAX_READ_CONNECTIONS);
        let metrics = Arc::new(Metrics::default());
        let blob_files = BlobFiles::new(root.join("blobs"))?;
        let store = Self {
            inner: Arc::new(Inner {
                root,
                writer: Mutex::new(db),
                readers: ReaderPool::open(&path, reader_count, Arc::clone(&metrics))?,
                blobs: BlobCache::new(
                    DEFAULT_BLOB_CACHE_BYTES,
                    DEFAULT_BLOB_CACHE_ENTRIES,
                    Arc::clone(&metrics),
                ),
                blob_files,
                metrics,
                temp_generation: AtomicU64::new(0),
                public_url,
                clock,
                expiry_defaults,
                #[cfg(test)]
                before_content_commit: Mutex::new(None),
            }),
        };
        store
            .migrate_sqlite_blobs()
            .map_err(|error| StoreError::startup("migrate SQLite blobs", error))?;
        store
            .restore_quarantined_blob_files()
            .map_err(|error| StoreError::startup("restore quarantined blobs", error))?;
        store
            .migrate_legacy()
            .map_err(|error| StoreError::startup("migrate legacy sites", error))?;
        store
            .backfill_manifests()
            .map_err(|error| StoreError::startup("backfill manifests", error))?;
        Ok(store)
    }

    #[cfg(test)]
    fn with_clock(
        root: PathBuf,
        public_url: String,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        Self::with_options(root, public_url, clock, DecayPolicy::default())
    }

    pub fn blocking_capacity(&self) -> usize {
        self.inner.readers.size() + 1
    }

    pub fn expiry_defaults(&self) -> DecayPolicy {
        self.inner.expiry_defaults
    }

    pub fn blob_path(&self, hash: &str) -> PathBuf {
        self.inner.blob_files.path(hash)
    }

    pub fn upload_path(&self) -> PathBuf {
        self.tmp_dir("upload")
    }

    pub fn stats(&self) -> Result<Stats, StoreError> {
        let mut db = self.inner.readers.get();
        let site_count = sites::table.select(count_star()).first::<i64>(&mut *db)?;
        let sites = site_count.cast_unsigned();
        let mut file_values = files::table
            .filter(files::path.ne(MANIFEST_PATH))
            .select(files::size)
            .load::<i64>(&mut *db)?
            .into_iter()
            .map(i64::cast_unsigned)
            .collect::<Vec<_>>();
        file_values.extend(
            allocated_entries::table
                .select(allocated_entries::size)
                .load::<i64>(&mut *db)?
                .into_iter()
                .map(i64::cast_unsigned),
        );
        file_values.sort_unstable();
        let mut referenced_blobs = files::table
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::hash, files::size))
            .load::<(String, i64)>(&mut *db)?;
        referenced_blobs.extend(
            allocated_entries::table
                .select((allocated_entries::hash, allocated_entries::size))
                .load::<(String, i64)>(&mut *db)?,
        );
        referenced_blobs.sort_unstable();
        referenced_blobs.dedup_by(|left, right| left.0 == right.0);
        let mut blob_values = referenced_blobs
            .into_iter()
            .map(|(_, size)| size.cast_unsigned())
            .collect::<Vec<_>>();
        blob_values.sort_unstable();
        let files = u64::try_from(file_values.len()).expect("file count fits in u64");
        let aliases = aliases::table
            .select(count_star())
            .first::<i64>(&mut *db)?
            .cast_unsigned();
        let blobs = u64::try_from(blob_values.len()).expect("blob count fits in u64");
        let logical_bytes = file_values.iter().sum();
        let bytes = blob_values.iter().sum();
        let saved_bytes = logical_bytes - bytes;
        let saved_fraction = if logical_bytes == 0 {
            0.0
        } else {
            u64_to_f64(saved_bytes) / u64_to_f64(logical_bytes)
        };
        drop(db);
        Ok(Stats {
            sites,
            files,
            aliases,
            blobs,
            bytes,
            logical_bytes,
            saved_bytes,
            saved_fraction,
            file_sizes: distribution(&file_values),
            blob_sizes: distribution(&blob_values),
            serving: self.inner.metrics.snapshot(),
        })
    }

    pub fn list_sites(&self) -> Result<SiteList, StoreError> {
        let mut db = self.inner.readers.get();
        let site_rows = sites::table
            .select((sites::id, sites::name))
            .order(sites::name)
            .load::<(i64, String)>(&mut *db)?;
        let mut entries = Vec::with_capacity(site_rows.len());
        for (site_id, name) in site_rows {
            let mut sizes = files::table
                .filter(files::site_id.eq(site_id))
                .filter(files::path.ne(MANIFEST_PATH))
                .select(files::size)
                .load::<i64>(&mut *db)?;
            sizes.extend(
                allocated_entries::table
                    .filter(allocated_entries::site_id.eq(site_id))
                    .select(allocated_entries::size)
                    .load::<i64>(&mut *db)?,
            );
            entries.push(SiteEnt {
                name,
                files: u64::try_from(sizes.len()).expect("file count fits in u64"),
                bytes: sizes.into_iter().map(i64::cast_unsigned).sum(),
            });
        }
        let files = entries.iter().map(|entry| entry.files).sum();
        let bytes = entries.iter().map(|entry| entry.bytes).sum();
        let alias_count = aliases::table
            .select(count_star())
            .first::<i64>(&mut *db)?
            .cast_unsigned();
        Ok(SiteList {
            files,
            alias_count,
            bytes,
            entries,
        })
    }

    #[cfg(test)]
    pub fn list_files(&self, name: &str) -> Result<Vec<String>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        if !site_exists_locked(&mut db, name)? {
            return Err(StoreError::NotFound);
        }
        let site_id = site_id_locked(&mut db, name)?;
        let mut paths = files::table
            .filter(files::site_id.eq(site_id))
            .select(files::path)
            .order(files::path)
            .load::<String>(&mut *db)?;
        paths.extend(
            allocated_entries::table
                .filter(allocated_entries::site_id.eq(site_id))
                .select(allocated_entries::path)
                .load::<String>(&mut *db)?,
        );
        paths.sort_unstable();
        Ok(paths)
    }

    pub fn list_dir(&self, name: &str, rel: &str) -> Result<DirList, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        if !rel.is_empty() && is_noise_path(Path::new(&rel)) {
            return Err(StoreError::NotFound);
        }
        let mut db = self.inner.readers.get();
        let mut snapshot = DbTransaction::begin(&mut db)?;
        match node_locked(&mut snapshot, name, &rel)? {
            NodeKind::Dir => {}
            NodeKind::File { .. } | NodeKind::Missing => return Err(StoreError::NotFound),
        }
        let alias_target = if rel.is_empty() {
            None
        } else {
            resolved_alias_directory_target_locked(&mut snapshot, name, &rel)?
        };
        let files = if let Some(ref target) = alias_target {
            load_alias_directory_files(&mut snapshot, name, &rel, target)?
        } else if rel.is_empty() {
            load_root_files(&mut snapshot, name)?
        } else {
            load_descendant_files(&mut snapshot, name, &rel)?
        };
        let aliases = load_directory_aliases(
            &mut snapshot,
            name,
            &rel,
            alias_target.as_deref().unwrap_or(&rel),
        )?;
        snapshot.commit()?;
        let mut listing = dirents(&files, &rel);
        listing.alias_count =
            u64::try_from(aliases.len()).expect("directory alias count fits in u64");
        listing.aliases = aliases;
        Ok(listing)
    }

    pub fn site_inventory(&self, name: &str) -> Result<SiteInventory, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let mut snapshot = DbTransaction::begin(&mut db)?;
        let (revision, tree_hash) = site_revision_locked(&mut snapshot, name)?;
        let site_id = site_id_locked(&mut snapshot, name)?;
        let rows = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .order(files::path)
            .load::<(String, String, i64)>(&mut *snapshot)?;
        let mut inventory = rows
            .into_iter()
            .map(|(path, hash, size)| InventoryFile {
                path,
                hash: format!("blake3:{hash}"),
                size: size.cast_unsigned(),
            })
            .collect::<Vec<_>>();
        let allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
            ))
            .load::<(String, String, i64)>(&mut *snapshot)?;
        for (path, hash, size) in allocated {
            inventory.push(InventoryFile {
                hash: format!("blake3:{hash}"),
                path,
                size: size.cast_unsigned(),
            });
        }
        inventory.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        let aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .order(aliases::path)
            .load::<AliasRow>(&mut *snapshot)?
            .into_iter()
            .map(|row| {
                alias_entry(row).map(|alias| InventoryAlias {
                    path: alias.path,
                    target: alias.canonical_target,
                    target_kind: alias.resolved_kind.map(|kind| match kind {
                        AliasResolvedKind::File => AliasTargetKind::File,
                        AliasResolvedKind::Directory => AliasTargetKind::Directory,
                    }),
                    dangling: alias.resolved_kind.is_none(),
                    resolved_hash: alias.resolved_hash.map(|hash| format!("blake3:{hash}")),
                    size: alias.resolved_size,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        snapshot.commit()?;
        Ok(SiteInventory {
            site: name.to_string(),
            content_revision: revision,
            tree_hash,
            files: inventory,
            aliases,
        })
    }

    pub fn site_exists(&self, name: &str) -> bool {
        let Ok(name) = parse_site_name(name) else {
            return false;
        };
        let mut db = self.inner.readers.get();
        site_exists_locked(&mut db, name).unwrap_or(false)
    }

    pub fn authorize_mutation(
        &self,
        name: &str,
        token: Option<&ManagementToken>,
    ) -> Result<(), StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        authorize_locked(&mut db, name, token)
    }

    pub fn management_status(&self, name: &str) -> Result<ManagementStatus, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let managed = sites::table
            .filter(sites::name.eq(name))
            .select(sites::management_status)
            .first::<i64>(&mut *db)
            .map_err(map_sql)?;
        Ok(ManagementStatus {
            managed: managed != 0,
        })
    }

    pub fn claim_management(
        &self,
        name: &str,
        creator: Option<CreatorIdentity>,
        claim: Option<&ClaimToken>,
        request: ManagementRequest<'_>,
    ) -> Result<ManagementMutation, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_management_idempotency(&mut tx, now)?;
        let fingerprint = format!("claim:{name}");
        let site = sites::table
            .filter(sites::name.eq(name))
            .select(ManagementSiteRow::as_select())
            .first::<ManagementSiteRow>(&mut *tx)
            .map_err(map_sql)?;
        let site_id = site.id;
        let managed = site.management_status != 0;
        let creator_kind = site.creator_kind;
        let creator_hash = site.creator_hash;
        let claim_hash = site.claim_hash;
        let creator_matches = creator.is_some_and(|candidate| {
            creator_kind == Some(candidate.kind as i64)
                && creator_hash.as_deref() == Some(candidate.hash.as_slice())
        });
        let claim_matches = match (claim_hash.as_deref(), claim) {
            (Some(hash), Some(candidate)) => {
                claim_hash_from_blob(hash).is_ok_and(|expected| expected.verify(candidate))
            }
            _ => false,
        };
        if !creator_matches && !claim_matches {
            return Err(StoreError::Forbidden);
        }
        if management_replay(&mut tx, request.idempotency, &fingerprint)? {
            return Ok(ManagementMutation {
                status: ManagementStatus { managed: true },
                token: None,
                replayed: true,
            });
        }
        if managed {
            return Err(StoreError::AlreadyManaged);
        }
        let token = ManagementToken::generate()?;
        diesel::update(sites::table.find(site_id))
            .set((
                sites::management_hash.eq(Some(token.hash().as_bytes().as_slice())),
                sites::management_status.eq(1_i64),
            ))
            .execute(&mut *tx)?;
        record_management(&mut tx, name, 1, now, request.audit_ip)?;
        store_management_idempotency(&mut tx, request.idempotency, &fingerprint, now)?;
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        tx.commit()?;
        drop(db);
        Ok(ManagementMutation {
            status: ManagementStatus { managed: true },
            token: Some(token),
            replayed: false,
        })
    }

    pub fn rotate_management(
        &self,
        name: &str,
        bearer: Option<&ManagementToken>,
        creator: Option<CreatorIdentity>,
        claim: Option<&ClaimToken>,
        request: ManagementRequest<'_>,
    ) -> Result<ManagementMutation, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_management_idempotency(&mut tx, now)?;
        let fingerprint = format!("rotate:{name}");
        let site = sites::table
            .filter(sites::name.eq(name))
            .select(ManagementSiteRow::as_select())
            .first::<ManagementSiteRow>(&mut *tx)
            .map_err(map_sql)?;
        let site_id = site.id;
        let managed = site.management_status != 0;
        let expected_hash = site.management_hash;
        let creator_kind = site.creator_kind;
        let creator_hash = site.creator_hash;
        let claim_hash = site.claim_hash;
        if !managed {
            return Err(StoreError::Forbidden);
        }
        let bearer_matches = match (expected_hash.as_deref(), bearer) {
            (Some(hash), Some(candidate)) => {
                management_hash_from_blob(hash).is_ok_and(|expected| expected.verify(candidate))
            }
            _ => false,
        };
        let creator_matches = creator.is_some_and(|candidate| {
            creator_kind == Some(candidate.kind as i64)
                && creator_hash.as_deref() == Some(candidate.hash.as_slice())
        });
        let claim_matches = match (claim_hash.as_deref(), claim) {
            (Some(hash), Some(candidate)) => {
                claim_hash_from_blob(hash).is_ok_and(|expected| expected.verify(candidate))
            }
            _ => false,
        };
        if !bearer_matches && !creator_matches && !claim_matches {
            return Err(StoreError::Unauthorized);
        }
        if management_replay(&mut tx, request.idempotency, &fingerprint)? {
            return Ok(ManagementMutation {
                status: ManagementStatus { managed: true },
                token: None,
                replayed: true,
            });
        }
        let token = ManagementToken::generate()?;
        diesel::update(sites::table.find(site_id))
            .set(sites::management_hash.eq(Some(token.hash().as_bytes().as_slice())))
            .execute(&mut *tx)?;
        record_management(&mut tx, name, 2, now, request.audit_ip)?;
        store_management_idempotency(&mut tx, request.idempotency, &fingerprint, now)?;
        tx.commit()?;
        drop(db);
        Ok(ManagementMutation {
            status: ManagementStatus { managed: true },
            token: Some(token),
            replayed: false,
        })
    }

    pub fn release_management(
        &self,
        name: &str,
        bearer: Option<&ManagementToken>,
        audit_ip: Option<&str>,
    ) -> Result<ManagementStatus, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, bearer)?;
        let site_id = site_id_locked(&mut tx, name)?;
        diesel::update(sites::table.find(site_id))
            .set((
                sites::management_hash.eq::<Option<Vec<u8>>>(None),
                sites::management_status.eq(0_i64),
            ))
            .execute(&mut *tx)?;
        diesel::delete(management_tombstones::table.find(name)).execute(&mut *tx)?;
        record_management(&mut tx, name, 3, now, audit_ip)?;
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        tx.commit()?;
        drop(db);
        Ok(ManagementStatus { managed: false })
    }

    pub fn operator_claim(&self, name: &str) -> Result<ManagementToken, StoreError> {
        let name = parse_site_name(name)?;
        let token = ManagementToken::generate()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let (site_id, status) = sites::table
            .filter(sites::name.eq(name))
            .select((sites::id, sites::management_status))
            .first::<(i64, i64)>(&mut *tx)
            .map_err(map_sql)?;
        let managed = status != 0;
        if managed {
            return Err(StoreError::AlreadyManaged);
        }
        diesel::update(sites::table.find(site_id))
            .set((
                sites::management_hash.eq(Some(token.hash().as_bytes().as_slice())),
                sites::management_status.eq(1_i64),
            ))
            .execute(&mut *tx)?;
        record_management(&mut tx, name, 4, now, None)?;
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        tx.commit()?;
        drop(db);
        Ok(token)
    }

    pub fn operator_rotate(
        &self,
        name: &str,
        current: &ManagementToken,
    ) -> Result<ManagementToken, StoreError> {
        let name = parse_site_name(name)?;
        let token = ManagementToken::generate()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, Some(current))?;
        let site_id = site_id_locked(&mut tx, name)?;
        diesel::update(sites::table.find(site_id))
            .set(sites::management_hash.eq(Some(token.hash().as_bytes().as_slice())))
            .execute(&mut *tx)?;
        record_management(&mut tx, name, 5, now, None)?;
        tx.commit()?;
        drop(db);
        Ok(token)
    }

    pub fn put_alias(
        &self,
        name: &str,
        path: &str,
        target: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        self.put_aliases(name, &[AliasSpec { path, target }], options)
    }

    pub fn put_aliases(
        &self,
        name: &str,
        specs: &[AliasSpec<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        self.put_aliases_with_receipt(name, specs, options)
            .map(|result| result.mutation)
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub fn put_aliases_with_receipt(
        &self,
        name: &str,
        specs: &[AliasSpec<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<AliasMutationResult, StoreError> {
        let name = parse_site_name(name)?;
        if specs.is_empty() {
            return Err(StoreError::InvalidAliasTarget);
        }
        let mut requested = BTreeMap::new();
        for spec in specs {
            if spec.path.starts_with(['/', '\\']) || spec.path.contains('\\') {
                return Err(StoreError::InvalidAliasTarget);
            }
            let path = normalize_rel(spec.path)?;
            if path.is_empty()
                || path.chars().any(char::is_control)
                || is_noise_path(Path::new(&path))
            {
                return Err(StoreError::InvalidAliasTarget);
            }
            reject_reserved_path(&path)?;
            let target = canonical_alias_target(&path, spec.target)?;
            match requested.insert(path.clone(), target.clone()) {
                Some(previous) if previous != target => return Err(StoreError::AliasConflict),
                _ => {}
            }
        }
        let fingerprint = alias_mutation_fingerprint(name, &requested, options.expected_tree_hash);
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(idempotency) = options.idempotency {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = alias_mutation_replay(&mut tx, idempotency, &fingerprint)? {
                return Ok(replay);
            }
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let entry_kinds = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .select((site_entries::path, site_entries::kind))
            .load::<(String, i64)>(&mut *tx)?
            .into_iter()
            .collect::<HashMap<_, _>>();
        let entry_paths = entry_kinds.keys().cloned().collect::<BTreeSet<_>>();
        let existing_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(&mut *tx)?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let prospective_alias_paths = existing_aliases
            .keys()
            .chain(requested.keys())
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut changed_paths = Vec::new();
        let mut created_paths = 0_usize;
        for (path, target) in &requested {
            if path
                .strip_prefix(target)
                .is_some_and(|suffix| suffix.starts_with('/'))
            {
                return Err(StoreError::AliasCycle);
            }
            if aggregate_paths(path)
                .into_iter()
                .any(|ancestor| !ancestor.is_empty() && prospective_alias_paths.contains(ancestor))
            {
                return Err(StoreError::AliasWrite);
            }
            let (descendant_start, descendant_end) = descendant_bounds(path);
            if entry_paths
                .range(descendant_start..descendant_end)
                .next()
                .is_some()
            {
                return Err(StoreError::AliasConflict);
            }
            let entry_kind = entry_kinds.get(path).copied();
            match entry_kind {
                Some(kind) if kind != database::schema::ALIAS_ENTRY_KIND => {
                    return Err(StoreError::AliasConflict);
                }
                Some(_) => {
                    let current = existing_aliases
                        .get(path)
                        .expect("alias entry has alias metadata");
                    if current != target {
                        changed_paths.push(path.as_str());
                    }
                }
                None => {
                    changed_paths.push(path.as_str());
                    created_paths += 1;
                }
            }
        }
        if changed_paths.is_empty() {
            let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
            let result = MutationResult {
                created: false,
                changed: false,
                replayed: false,
                files: requested.len(),
                revision,
                tree_hash,
                undo: None,
                sanitized: TokenCounts::default(),
            };
            let result = AliasMutationResult {
                aliases: load_requested_aliases_locked(&mut tx, site_id, requested.keys())?,
                mutation: result,
            };
            store_alias_mutation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
            tx.commit()?;
            return Ok(result);
        }
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            UndoKind::Alias,
            &format!("restore previous aliases in {name}"),
            &changed_paths,
            now,
        )?;
        for path in &changed_paths {
            let target = &requested[*path];
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(*path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .on_conflict_do_nothing()
                .execute(&mut *tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(site_id),
                    aliases::path.eq(*path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(target),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<String>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .on_conflict((aliases::site_id, aliases::path))
                .do_update()
                .set((
                    aliases::canonical_target.eq(excluded(aliases::canonical_target)),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<String>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .execute(&mut *tx)?;
        }
        diesel::update(sites::table.find(site_id))
            .set((
                sites::updated.eq(now),
                sites::content_revision.eq(sites::content_revision + 1),
            ))
            .execute(&mut *tx)?;
        let alias_changes = changed_paths
            .iter()
            .map(|path| AliasChange::Alias(path))
            .collect::<Vec<_>>();
        refresh_aliases_locked(&mut tx, site_id, &alias_changes)?;
        refresh_expiry_for_changes_locked(&mut tx, site_id, &changed_paths, now)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        let (revision, _) = site_revision_locked(&mut tx, name)?;
        prune_undo_locked(&mut tx, now)?;
        let result = MutationResult {
            created: created_paths == changed_paths.len(),
            changed: true,
            replayed: false,
            files: changed_paths.len(),
            revision,
            tree_hash,
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        };
        let result = AliasMutationResult {
            aliases: load_requested_aliases_locked(&mut tx, site_id, requested.keys())?,
            mutation: result,
        };
        store_alias_mutation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn alias(&self, name: &str, path: &str) -> Result<AliasEntry, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        let row = aliases::table
            .find((site_id, path.as_str()))
            .select(AliasRow::as_select())
            .first::<AliasRow>(&mut *db)
            .map_err(map_sql)?;
        alias_entry(row)
    }

    pub fn aliases(&self, name: &str) -> Result<Vec<AliasEntry>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .order(aliases::path)
            .load::<AliasRow>(&mut *db)?
            .into_iter()
            .map(alias_entry)
            .collect()
    }

    pub fn alias_inventory(&self, name: &str) -> Result<AliasInventory, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let mut snapshot = DbTransaction::begin(&mut db)?;
        let (site_id, content_revision, tree_hash) = sites::table
            .filter(sites::name.eq(name))
            .select((sites::id, sites::content_revision, sites::tree_hash))
            .first::<(i64, i64, String)>(&mut *snapshot)
            .map_err(map_sql)?;
        let aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .order(aliases::path)
            .load::<AliasRow>(&mut *snapshot)?
            .into_iter()
            .map(alias_entry)
            .collect::<Result<Vec<_>, _>>()?;
        snapshot.commit()?;
        Ok(AliasInventory {
            site: name.to_string(),
            content_revision: content_revision.cast_unsigned(),
            tree_hash,
            aliases,
        })
    }

    pub fn alias_stats(&self, name: &str) -> Result<AliasStats, StoreError> {
        let inventory = self.alias_inventory(name)?;
        let aliases = u64::try_from(inventory.aliases.len()).expect("alias count fits in u64");
        let resolved = u64::try_from(
            inventory
                .aliases
                .iter()
                .filter(|alias| alias.resolved_kind.is_some())
                .count(),
        )
        .expect("resolved alias count fits in u64");
        Ok(AliasStats {
            aliases,
            resolved,
            dangling: aliases - resolved,
        })
    }

    pub fn lookup(&self, name: &str, rel: &str) -> Result<Node, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        if !rel.is_empty() && is_noise_path(Path::new(&rel)) {
            return Err(StoreError::NotFound);
        }
        let mut db = self.inner.readers.get();
        let node = match node_locked(&mut db, name, &rel)? {
            NodeKind::Missing => return Err(StoreError::NotFound),
            NodeKind::Dir => Node::Dir,
            NodeKind::File { hash } => Node::File { logical: rel, hash },
        };
        Ok(node)
    }

    pub fn child_blob(&self, name: &str, rel: &str, child: &str) -> Result<Node, StoreError> {
        let path = if rel.is_empty() {
            child.to_string()
        } else {
            format!("{rel}/{child}")
        };
        self.lookup(name, &path)
    }

    pub fn read_blob(&self, hash: &str) -> Result<Bytes, StoreError> {
        if let Some(bytes) = self.inner.blobs.get(hash) {
            return Ok(bytes);
        }
        let mut db = self.inner.readers.get();
        blobs::table
            .find(hash)
            .select(blobs::hash)
            .first::<String>(&mut *db)
            .map_err(map_sql)?;
        drop(db);
        let bytes = Bytes::from(self.inner.blob_files.read(hash)?);
        self.inner.blobs.insert(hash, bytes.clone());
        Ok(bytes)
    }

    pub fn allocated_media_type(
        &self,
        name: &str,
        rel: &str,
    ) -> Result<Option<String>, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        let direct = allocated_entries::table
            .find((site_id, rel.as_str()))
            .select(allocated_entries::media_type)
            .first::<String>(&mut *db)
            .optional()?;
        if direct.is_some() {
            return Ok(direct);
        }
        let (resolution, physical, used_alias) = resolve_db_path_final(&mut db, site_id, &rel)?;
        if !used_alias || !matches!(resolution, GraphResolution::File(_)) {
            return Ok(None);
        }
        allocated_entries::table
            .find((site_id, physical))
            .select(allocated_entries::media_type)
            .first::<String>(&mut *db)
            .optional()
            .map_err(StoreError::from)
    }

    pub fn site_references_blob(&self, name: &str, hash: &str) -> Result<bool, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let count = files::table
            .inner_join(sites::table)
            .filter(sites::name.eq(name))
            .filter(files::hash.eq(hash))
            .select(count_star())
            .first::<i64>(&mut *db)?;
        if count != 0 {
            return Ok(true);
        }
        let site_id = site_id_locked(&mut db, name)?;
        let count = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::hash.eq(hash))
            .select(count_star())
            .first::<i64>(&mut *db)?;
        Ok(count != 0)
    }

    #[allow(clippy::large_types_passed_by_value)]
    pub fn publish_uploaded_archive(
        &self,
        wanted: Option<&str>,
        filename: Option<&str>,
        source: &Path,
        kind: Kind,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let tmp = self.tmp_dir(wanted.unwrap_or("upload"));
        fs::create_dir_all(&tmp)?;
        let result = (|| {
            let plan = plan_archive(source, kind)?;
            if plan
                .members
                .iter()
                .any(|member| matches!(member, ArchiveMember::File { .. }))
            {
                write_payload_file(&tmp, source, kind, filename, true)?;
            }
            let staged = stage_dir(&tmp)?;
            if staged.is_empty()
                && !plan
                    .members
                    .iter()
                    .any(|member| matches!(member, ArchiveMember::Alias { .. }))
            {
                return Err(StoreError::Upload(UploadError::EmptyArchive));
            }
            self.publish_archive_staged(wanted, &staged, &plan, options)
        })();
        let _ = fs::remove_dir_all(tmp);
        result
    }

    #[allow(clippy::large_types_passed_by_value)]
    pub fn publish_uploaded_file(
        &self,
        wanted: Option<&str>,
        filename: &str,
        source: PathBuf,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let rel = safe_rel_path(filename)?
            .to_string_lossy()
            .replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        self.publish_staged(wanted, std::slice::from_ref(&staged), options)
    }

    #[cfg(test)]
    pub fn replace_site(
        &self,
        name: &str,
        bytes: &[u8],
        kind: Kind,
        filename: Option<&str>,
        unpack: bool,
    ) -> Result<usize, StoreError> {
        let name = parse_site_name(name)?.to_string();
        let tmp = self.tmp_dir(&name);
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        match write_payload(&tmp, bytes, kind, filename, unpack) {
            Ok(_) => {}
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        }
        let staged = match stage_dir(&tmp) {
            Ok(files) if !files.is_empty() => files,
            Ok(_) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(UploadError::EmptyArchive.into());
            }
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        };
        let n = staged.len();
        let result = self.merge_staged(&name, &staged, UndoKind::Put);
        let _ = fs::remove_dir_all(&tmp);
        result?;
        Ok(n)
    }

    #[cfg(test)]
    pub fn put_file(&self, name: &str, rel: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        if is_junk(Path::new(&rel), Some(bytes)) {
            return Err(UploadError::Junk.into());
        }
        let staged = stage_bytes(&rel, bytes);
        self.upsert_file(&name, &staged).map(|_| ())
    }

    #[cfg(test)]
    pub fn put_uploaded_file(
        &self,
        name: &str,
        rel: &str,
        source: PathBuf,
        expected_tree_hash: Option<&str>,
    ) -> Result<MutationResult, StoreError> {
        self.put_uploaded_file_secured(
            name,
            rel,
            source,
            PublishOptions {
                expected_tree_hash,
                ..PublishOptions::default()
            },
        )
    }

    #[allow(clippy::large_types_passed_by_value)]
    pub fn put_uploaded_file_secured(
        &self,
        name: &str,
        rel: &str,
        source: PathBuf,
        options: PublishOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        reject_reserved_path(&staged.path)?;
        self.merge_staged_conditional(
            &name,
            std::slice::from_ref(&staged),
            UndoKind::PutFile,
            options.expected_tree_hash,
            options.creation,
            options.authorization,
        )
    }

    pub fn allocate_bytes(
        &self,
        name: &str,
        bytes: &[u8],
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.allocate_source(name, AllocationSource::Bytes(bytes), spec, options)
    }

    pub fn allocate_file(
        &self,
        name: &str,
        source: &Path,
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.allocate_source(name, AllocationSource::File(source), spec, options)
    }

    pub fn allocate_source(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let staged = self.stage_allocation_source(source, "")?;
        let destination = allocation_destination(&staged.hash, spec)?;
        self.run_before_content_commit();
        self.commit_allocated(
            name,
            None,
            &destination,
            &staged,
            None,
            None,
            options,
            UndoKind::Allocate,
        )
    }

    pub fn replace_allocated(
        &self,
        name: &str,
        current_path: &str,
        source: AllocationSource<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let current_path = normalize_rel(current_path)?;
        let staged = self.stage_allocation_source(source, "")?;
        let request_fingerprint = content_mutation_request_fingerprint(
            name,
            &current_path,
            &staged.hash,
            None,
            options.expected_tree_hash,
            UndoKind::Replace,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let metadata = self.allocated_metadata(name, &current_path)?;
        let destination = relocated_destination(&staged.hash, &current_path, &metadata)?;
        self.run_before_content_commit();
        self.commit_allocated(
            name,
            Some(&current_path),
            &destination,
            &staged,
            None,
            None,
            options,
            UndoKind::Replace,
        )
    }

    pub fn propose_allocation(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: PendingAllocationSpec<'_>,
        authorization: Option<&ManagementToken>,
    ) -> Result<PendingAllocation, StoreError> {
        self.propose_allocation_idempotent(
            name,
            source,
            spec,
            FileMutationOptions {
                authorization,
                ..FileMutationOptions::default()
            },
        )
    }

    pub fn propose_allocation_idempotent(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: PendingAllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<PendingAllocation, StoreError> {
        let name = parse_site_name(name)?;
        let staged = self.stage_allocation_source(source, "")?;
        let folder = normalize_folder(spec.folder)?;
        let media_type = normalize_media_type(spec.media_type)?;
        let extension = spec
            .extension
            .map(normalize_allocated_extension)
            .transpose()?;
        let expiry = validate_file_expiry(options.expiry)?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let request_fingerprint = pending_fingerprint(&PendingFingerprint {
            site: name,
            folder: &folder,
            hash: &staged.hash,
            content_size: staged.size.cast_unsigned(),
            media_type: &media_type,
            expiry,
            authorization_hash: authorization_hash.as_deref(),
            extension: extension.as_deref(),
            expected_tree_hash: options.expected_tree_hash,
        });
        let now = self.now_millis();
        let expires = now + PENDING_RETENTION_MILLIS;
        self.run_before_content_commit();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        prune_idempotency_locked(&mut tx, now)?;
        prune_pending_locked(&mut tx, now)?;
        if let Some(replay) = pending_allocation_replay(
            &mut tx,
            options.idempotency,
            &request_fingerprint,
            site_id,
            now,
        )? {
            tx.commit()?;
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let token = undo_token()?;
        self.materialize(&staged)?;
        ensure_blob_locked(&mut tx, &staged)?;
        let request_metadata = serde_json::to_string(&PendingRequestMetadata {
            request_fingerprint: request_fingerprint.clone(),
            expiry,
            authorization_hash,
            extension: extension.clone(),
            expected_tree_hash: options.expected_tree_hash.map(str::to_string),
            legacy_fingerprint: false,
            sanitized: staged.sanitized,
        })
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        diesel::insert_into(pending_allocations::table)
            .values((
                pending_allocations::token.eq(&token),
                pending_allocations::site_id.eq(site_id),
                pending_allocations::folder.eq(&folder),
                pending_allocations::hash.eq(&staged.hash),
                pending_allocations::size.eq(staged.size),
                pending_allocations::media_type.eq(&media_type),
                pending_allocations::request_fingerprint.eq(request_metadata),
                pending_allocations::created.eq(now),
                pending_allocations::expires.eq(expires),
            ))
            .execute(&mut *tx)?;
        let (content_revision, tree_hash) = site_revision_locked(&mut tx, name)?;
        let result = PendingAllocation {
            token,
            folder,
            hash: staged.hash.clone(),
            size: staged.size.cast_unsigned(),
            media_type,
            extension,
            expires_at: format_timestamp(expires),
            tree_hash,
            content_revision,
            replayed: false,
        };
        store_pending_allocation(
            &mut tx,
            options.idempotency,
            &request_fingerprint,
            &result,
            now,
        )?;
        tx.commit()?;
        drop(db);
        Ok(result)
    }

    pub fn finalize_allocation(
        &self,
        name: &str,
        token: &str,
        naming: AllocatedName<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            None,
            PendingFinalName::Generated(naming),
            options,
        )
    }

    pub fn finalize_allocation_custom(
        &self,
        name: &str,
        token: &str,
        basename: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            None,
            PendingFinalName::Custom(basename),
            options,
        )
    }

    pub fn finalize_allocation_custom_in_folder(
        &self,
        name: &str,
        token: &str,
        folder: &str,
        basename: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            Some(folder),
            PendingFinalName::Custom(basename),
            options,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn finalize_pending(
        &self,
        name: &str,
        token: &str,
        expected_folder: Option<&str>,
        final_name: PendingFinalName<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        let expected_folder = expected_folder.map(normalize_folder).transpose()?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let finalize_fingerprint = pending_finalize_fingerprint(&PendingFinalizeFingerprint {
            site: name,
            token,
            folder: expected_folder.as_deref(),
            final_name,
            expiry: options.expiry,
            authorization_hash: authorization_hash.as_deref(),
            expected_tree_hash: options.expected_tree_hash,
        });
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) =
            entry_mutation_replay(&mut tx, options.idempotency, &finalize_fingerprint)?
        {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .filter(pending_allocations::site_id.eq(site_id))
            .filter(pending_allocations::expires.gt(now))
            .select((
                pending_allocations::folder,
                pending_allocations::hash,
                pending_allocations::size,
                pending_allocations::media_type,
                pending_allocations::request_fingerprint,
            ))
            .first::<(String, String, i64, String, String)>(&mut *tx)
            .optional()?
            .map(|row| {
                let request = parse_pending_request_metadata(&row.4)?;
                Ok::<_, StoreError>(PendingMetadata {
                    folder: row.0,
                    hash: row.1,
                    size: row.2,
                    media_type: row.3,
                    request_fingerprint: request.request_fingerprint,
                    expiry: request.expiry,
                    authorization_hash: request.authorization_hash,
                    extension: request.extension,
                    expected_tree_hash: request.expected_tree_hash,
                    legacy_fingerprint: request.legacy_fingerprint,
                    sanitized: request.sanitized,
                })
            })
            .transpose()?
            .ok_or(StoreError::InvalidPendingAllocation)?;
        if expected_folder
            .as_deref()
            .is_some_and(|expected| expected != pending.folder)
        {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let expected_pending_fingerprint = if pending.legacy_fingerprint {
            legacy_pending_fingerprint(
                name,
                &pending.folder,
                &pending.hash,
                pending.size.cast_unsigned(),
                &pending.media_type,
            )
        } else {
            pending_fingerprint(&PendingFingerprint {
                site: name,
                folder: &pending.folder,
                hash: &pending.hash,
                content_size: pending.size.cast_unsigned(),
                media_type: &pending.media_type,
                expiry: pending.expiry,
                authorization_hash: pending.authorization_hash.as_deref(),
                extension: pending.extension.as_deref(),
                expected_tree_hash: pending.expected_tree_hash.as_deref(),
            })
        };
        if pending.request_fingerprint != expected_pending_fingerprint {
            return Err(StoreError::InvalidPendingAllocation);
        }
        if !pending.legacy_fingerprint && pending.authorization_hash != authorization_hash {
            return Err(StoreError::Unauthorized);
        }
        if pending.expiry != FileExpiry::Preserve
            && options.expiry != FileExpiry::Preserve
            && options.expiry != pending.expiry
        {
            return Err(StoreError::IdempotencyConflict);
        }
        let options = FileMutationOptions {
            expiry: validate_file_expiry(if pending.expiry == FileExpiry::Preserve {
                options.expiry
            } else {
                pending.expiry
            })?,
            ..options
        };
        let destination = match final_name {
            PendingFinalName::Generated(naming) => allocation_destination(
                &pending.hash,
                AllocationSpec {
                    folder: &pending.folder,
                    naming,
                    media_type: &pending.media_type,
                },
            )?,
            PendingFinalName::Custom(basename) => {
                custom_destination(&pending.folder, basename, &pending.media_type)?
            }
        };
        let staged = StagedFile {
            path: destination.path.clone(),
            size: pending.size,
            hash: pending.hash.clone(),
            source: StagedSource::File(self.inner.blob_files.path(&pending.hash)),
            sanitized: pending.sanitized,
        };
        let result = self.commit_allocated_locked(
            &mut tx,
            name,
            None,
            &destination,
            &staged,
            None,
            options,
            UndoKind::Allocate,
            now,
        )?;
        store_entry_mutation(
            &mut tx,
            options.idempotency,
            &finalize_fingerprint,
            &result,
            now,
        )?;
        diesel::delete(pending_allocations::table.find(token)).execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    pub fn cancel_allocation(
        &self,
        name: &str,
        token: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<(), StoreError> {
        if self.cancel_allocation_in_folder(name, token, None, authorization)? {
            Ok(())
        } else {
            Err(StoreError::InvalidPendingAllocation)
        }
    }

    pub fn cancel_allocation_idempotent_for_folder(
        &self,
        name: &str,
        token: &str,
        folder: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocationCancellation, StoreError> {
        let name = parse_site_name(name)?;
        let folder = normalize_folder(folder)?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let fingerprint = cancellation_fingerprint(
            name,
            token,
            &folder,
            options.expected_tree_hash,
            authorization_hash.as_deref(),
        );
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) = cancellation_replay(&mut tx, options.idempotency, &fingerprint)? {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .select((
                pending_allocations::site_id,
                pending_allocations::folder,
                pending_allocations::request_fingerprint,
            ))
            .first::<(i64, String, String)>(&mut *tx)
            .optional()?
            .ok_or(StoreError::InvalidPendingAllocation)?;
        if pending.0 != site_id || pending.1 != folder {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let request = parse_pending_request_metadata(&pending.2)?;
        if !request.legacy_fingerprint && request.authorization_hash != authorization_hash {
            return Err(StoreError::Unauthorized);
        }
        let deleted = diesel::delete(
            pending_allocations::table
                .find(token)
                .filter(pending_allocations::site_id.eq(site_id)),
        )
        .execute(&mut *tx)?;
        if deleted != 1 {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let (revision, tree_hash) = sites::table
            .find(site_id)
            .select((sites::content_revision, sites::tree_hash))
            .first::<(i64, String)>(&mut *tx)?;
        let result = AllocationCancellation {
            replayed: false,
            tree_hash,
            content_revision: revision.cast_unsigned(),
        };
        store_cancellation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    fn cancel_allocation_in_folder(
        &self,
        name: &str,
        token: &str,
        folder: Option<&str>,
        authorization: Option<&ManagementToken>,
    ) -> Result<bool, StoreError> {
        let name = parse_site_name(name)?;
        let folder = folder.map(normalize_folder).transpose()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .select((
                pending_allocations::site_id,
                pending_allocations::folder,
                pending_allocations::request_fingerprint,
            ))
            .first::<(i64, String, String)>(&mut *tx)
            .optional()?;
        let Some((stored_site_id, stored_folder, request_metadata)) = pending else {
            return Ok(false);
        };
        if stored_site_id != site_id {
            return Err(StoreError::InvalidPendingAllocation);
        }
        if folder
            .as_deref()
            .is_some_and(|expected| expected != stored_folder)
        {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let request = parse_pending_request_metadata(&request_metadata)?;
        if !request.legacy_fingerprint
            && request.authorization_hash != authorization.map(authorization_fingerprint)
        {
            return Err(StoreError::Unauthorized);
        }
        diesel::delete(
            pending_allocations::table
                .find(token)
                .filter(pending_allocations::site_id.eq(site_id)),
        )
        .execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(true)
    }

    pub fn prune_pending_allocations(&self) -> Result<usize, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let removed_pending = prune_pending_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(removed_pending)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_file_content(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        source: AllocationSource<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        reject_reserved_path(&path)?;
        let staged = self.stage_allocation_source(source, &path)?;
        let request_fingerprint = content_mutation_request_fingerprint(
            name,
            &path,
            &staged.hash,
            Some(base_hash),
            options.expected_tree_hash,
            UndoKind::Replace,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        reject_alias_write_locked(&mut db, site_id, &path, false)?;
        let kind = site_entries::table
            .find((site_id, path.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *db)
            .map_err(map_sql)?;
        let current_hash = entry_hash_locked(&mut db, site_id, &path, kind)?;
        drop(db);
        if current_hash != base_hash {
            return Err(StoreError::StaleContentHash(current_hash));
        }
        if kind == database::schema::ALLOCATED_ENTRY_KIND {
            let metadata = self.allocated_metadata(name, &path)?;
            let destination = relocated_destination(&staged.hash, &path, &metadata)?;
            self.run_before_content_commit();
            self.commit_allocated(
                name,
                Some(&path),
                &destination,
                &staged,
                Some(base_hash),
                None,
                options,
                UndoKind::Replace,
            )
        } else {
            self.run_before_content_commit();
            self.commit_regular(
                name,
                &path,
                &staged,
                Some(base_hash),
                None,
                options,
                UndoKind::Replace,
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn splice_file(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        splices: &[Splice<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.splice_file_with_limit(
            name,
            path,
            base_hash,
            splices,
            options,
            MAX_SPLICE_RESULT_SIZE,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn splice_file_with_limit(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        splices: &[Splice<'_>],
        options: FileMutationOptions<'_>,
        maximum_result_size: u64,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        reject_reserved_path(&path)?;
        let temporary = TemporaryDirectory::create(self.tmp_dir("splice"))?;
        let output = temporary.path().join("result");
        let prepared = prepare_splices(splices, temporary.path(), maximum_result_size)?;
        let request_fingerprint = splice_request_fingerprint(
            name,
            &path,
            base_hash,
            &prepared,
            options.expected_tree_hash,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        reject_alias_write_locked(&mut db, site_id, &path, false)?;
        let kind = site_entries::table
            .find((site_id, path.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *db)
            .map_err(map_sql)?;
        let current_hash = entry_hash_locked(&mut db, site_id, &path, kind)?;
        if current_hash != base_hash {
            return Err(StoreError::StaleContentHash(current_hash));
        }
        let old_size = entry_size_locked(&mut db, site_id, &path, kind)?;
        drop(db);
        self.run_before_content_commit();
        splice_blob_to_path(
            &self.inner.blob_files.path(base_hash),
            old_size,
            &prepared,
            &output,
            maximum_result_size,
        )?;
        let mut staged = stage_borrowed_file(&path, output)?;
        for splice in &prepared {
            staged.sanitized.management += splice.sanitized.management;
            staged.sanitized.claim += splice.sanitized.claim;
        }
        if kind == database::schema::ALLOCATED_ENTRY_KIND {
            let metadata = self.allocated_metadata(name, &path)?;
            let destination = relocated_destination(&staged.hash, &path, &metadata)?;
            self.run_before_content_commit();
            self.commit_allocated(
                name,
                Some(&path),
                &destination,
                &staged,
                Some(base_hash),
                Some(&request_fingerprint),
                options,
                UndoKind::Splice,
            )
        } else {
            self.run_before_content_commit();
            self.commit_regular(
                name,
                &path,
                &staged,
                Some(base_hash),
                Some(&request_fingerprint),
                options,
                UndoKind::Splice,
            )
        }
    }

    #[cfg(test)]
    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let archive = site_files(&mut tx, &self.inner.blob_files, name)?;
        let packed = pack_tar_gz(&archive.files)?;
        snapshot_site(&mut tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&mut tx, name, self.now_millis())?;
        diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(packed)
    }

    #[cfg(test)]
    pub fn pack_site(&self, name: &str, format: ArchiveFormat) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let archive = site_files(&mut db, &self.inner.blob_files, name)?;
        drop(db);
        match format {
            ArchiveFormat::Tar => pack_tar(&archive.files),
            ArchiveFormat::TarGz => pack_tar_gz(&archive.files),
            ArchiveFormat::Zip => pack_zip(&archive.files),
        }
        .map_err(StoreError::Io)
    }

    pub fn pack_site_to_path(
        &self,
        name: &str,
        format: ArchiveFormat,
        output: &Path,
    ) -> Result<u64, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let entries = site_manifest(&mut db, name)?;
        drop(db);
        write_site_archive(&self.inner.blob_files, &entries, format, output)?;
        Ok(fs::metadata(output)?.len())
    }

    pub fn pop_site_to_path_secured(
        &self,
        name: &str,
        format: ArchiveFormat,
        output: &Path,
        authorization: Option<&ManagementToken>,
    ) -> Result<PopResult, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let entries = site_manifest(&mut db, name)?;
        write_site_archive(&self.inner.blob_files, &entries, format, output)?;
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let undo = snapshot_site(&mut tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&mut tx, name, self.now_millis())?;
        diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        prune_undo_locked(&mut tx, self.now_millis())?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(PopResult {
            size: fs::metadata(output)?.len(),
            undo,
        })
    }

    #[cfg(test)]
    pub fn copy_site(
        &self,
        source: &str,
        destination: Option<&str>,
        idempotency: Option<&Idempotency>,
    ) -> Result<(String, MutationResult), StoreError> {
        self.copy_site_secured(
            source,
            destination,
            idempotency,
            CreationSecurity::default(),
        )
    }

    #[allow(clippy::too_many_lines)]
    pub fn copy_site_secured(
        &self,
        source: &str,
        destination: Option<&str>,
        idempotency: Option<&Idempotency>,
        creation: CreationSecurity,
    ) -> Result<(String, MutationResult), StoreError> {
        let source = parse_site_name(source)?;
        let destination = destination
            .map(parse_site_name)
            .transpose()?
            .map(str::to_string);
        let now = self.now_millis();
        let fingerprint = format!("copy:{source}");
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_idempotency_locked(&mut tx, now)?;
        if !site_exists_locked(&mut tx, source)? {
            return Err(StoreError::NotFound);
        }
        if destination.is_none()
            && let Some(idempotency) = idempotency
        {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let generated = destination.is_none();
        let existing_names = sites::table
            .select(sites::name)
            .load::<String>(&mut *tx)?
            .into_iter()
            .collect::<HashSet<_>>();
        let destination = destination
            .unwrap_or_else(|| generate_id(|candidate| existing_names.contains(candidate)));
        if site_exists_locked(&mut tx, &destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let (source_id, public_url, revision) = sites::table
            .filter(sites::name.eq(source))
            .select((sites::id, sites::public_url, sites::content_revision))
            .first::<(i64, String, i64)>(&mut *tx)?;
        let undo = snapshot_site_with_description(
            &mut tx,
            &destination,
            UndoKind::Copy,
            &format!("remove copied site {destination}"),
            now,
        )?;
        let destination_id = diesel::insert_into(sites::table)
            .values(NewSite {
                name: &destination,
                updated: now,
                public_url: &public_url,
                content_revision: revision,
                tree_hash: "",
                creator_kind: creation.creator.map(|creator| creator.kind as i64),
                creator_hash: creation.creator.map(|creator| creator.hash.to_vec()),
                claim_hash: creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                management_hash: creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                management_status: i64::from(creation.management_hash.is_some()),
            })
            .returning(sites::id)
            .get_result::<i64>(&mut *tx)?;
        let copied_files = files::table
            .filter(files::site_id.eq(source_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .load::<(String, String, i64)>(&mut *tx)?;
        for (path, hash, size) in copied_files {
            ensure_file_entry(&mut tx, destination_id, &path)?;
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id: destination_id,
                    path: &path,
                    hash: &hash,
                    size,
                })
                .execute(&mut *tx)?;
        }
        let copied_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(source_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
                allocated_entries::naming_mode,
                allocated_entries::prefix,
                allocated_entries::suffix,
                allocated_entries::extension,
                allocated_entries::media_type,
            ))
            .load::<(
                String,
                String,
                i64,
                i64,
                String,
                String,
                Option<String>,
                String,
            )>(&mut *tx)?;
        for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
            copied_allocated
        {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(destination_id),
                    site_entries::path.eq(&path),
                    site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                ))
                .execute(&mut *tx)?;
            diesel::insert_into(allocated_entries::table)
                .values((
                    allocated_entries::site_id.eq(destination_id),
                    allocated_entries::path.eq(path),
                    allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    allocated_entries::hash.eq(hash),
                    allocated_entries::size.eq(size),
                    allocated_entries::naming_mode.eq(naming_mode),
                    allocated_entries::prefix.eq(prefix),
                    allocated_entries::suffix.eq(suffix),
                    allocated_entries::extension.eq(extension),
                    allocated_entries::media_type.eq(media_type),
                ))
                .execute(&mut *tx)?;
        }
        let copied_aliases = aliases::table
            .filter(aliases::site_id.eq(source_id))
            .select((
                aliases::path,
                aliases::canonical_target,
                aliases::resolved_kind,
                aliases::resolved_hash,
                aliases::resolved_size,
            ))
            .load::<(String, String, Option<i64>, Option<String>, Option<i64>)>(&mut *tx)?;
        for (path, target, resolved_kind, resolved_hash, resolved_size) in copied_aliases {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(destination_id),
                    site_entries::path.eq(&path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .execute(&mut *tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(destination_id),
                    aliases::path.eq(path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(target),
                    aliases::resolved_kind.eq(resolved_kind),
                    aliases::resolved_hash.eq(resolved_hash),
                    aliases::resolved_size.eq(resolved_size),
                ))
                .execute(&mut *tx)?;
        }
        let aggregates = path_aggregates::table
            .filter(path_aggregates::site_id.eq(source_id))
            .select((
                path_aggregates::path,
                path_aggregates::logical_bytes,
                path_aggregates::file_count,
            ))
            .load::<(String, i64, i64)>(&mut *tx)?;
        for (path, logical_bytes, file_count) in aggregates {
            diesel::insert_into(path_aggregates::table)
                .values((
                    path_aggregates::site_id.eq(destination_id),
                    path_aggregates::path.eq(path),
                    path_aggregates::logical_bytes.eq(logical_bytes),
                    path_aggregates::file_count.eq(file_count),
                ))
                .execute(&mut *tx)?;
        }
        copy_expiry_policies_locked(&mut tx, source_id, destination_id, now)?;
        let files = site_entries::table
            .filter(site_entries::site_id.eq(destination_id))
            .filter(site_entries::path.ne(MANIFEST_PATH))
            .filter(site_entries::kind.ne(database::schema::ALIAS_ENTRY_KIND))
            .select(count_star())
            .first::<i64>(&mut *tx)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, destination_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        let mutation = MutationResult {
            created: true,
            changed: true,
            replayed: false,
            files: usize::try_from(files).expect("file count fits in usize"),
            revision: revision.cast_unsigned(),
            tree_hash,
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        };
        if generated && let Some(idempotency) = idempotency {
            let published = PublishedMutation {
                name: destination.clone(),
                mutation: mutation.clone(),
            };
            store_idempotency(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((destination, mutation))
    }

    #[cfg(test)]
    pub fn move_site(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<(String, MutationResult), StoreError> {
        self.move_site_secured(source, destination, None)
    }

    pub fn move_site_secured(
        &self,
        source: &str,
        destination: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<(String, MutationResult), StoreError> {
        let source = parse_site_name(source)?;
        let destination = parse_site_name(destination)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, source, authorization)?;
        if !site_exists_locked(&mut tx, source)? {
            return Err(StoreError::NotFound);
        }
        if site_exists_locked(&mut tx, destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let undo = snapshot_site_with_description(
            &mut tx,
            source,
            UndoKind::Move,
            &format!("move {destination} back to {source}"),
            now,
        )?;
        diesel::insert_into(undo_names::table)
            .values((
                undo_names::token.eq(&undo.token),
                undo_names::name.eq(destination),
            ))
            .execute(&mut *tx)?;
        diesel::update(sites::table.filter(sites::name.eq(source)))
            .set((sites::name.eq(destination), sites::updated.eq(now)))
            .execute(&mut *tx)?;
        let (site_id, revision) = sites::table
            .filter(sites::name.eq(destination))
            .select((sites::id, sites::content_revision))
            .first::<(i64, i64)>(&mut *tx)?;
        let files = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .filter(site_entries::path.ne(MANIFEST_PATH))
            .filter(site_entries::kind.ne(database::schema::ALIAS_ENTRY_KIND))
            .select(count_star())
            .first::<i64>(&mut *tx)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((
            destination.to_string(),
            MutationResult {
                created: false,
                changed: true,
                replayed: false,
                files: usize::try_from(files).expect("file count fits in usize"),
                revision: revision.cast_unsigned(),
                tree_hash,
                undo: Some(undo),
                sanitized: TokenCounts::default(),
            },
        ))
    }

    #[cfg(test)]
    pub fn delete_file(&self, name: &str, rel: &str) -> Result<MutationResult, StoreError> {
        self.delete_file_secured(name, rel, None)
    }

    pub fn delete_file_secured(
        &self,
        name: &str,
        rel: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<MutationResult, StoreError> {
        let name = parse_site_name(name)?;
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let (prefix_start, prefix_end) = descendant_bounds(&rel);
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        reject_reserved_path(&rel)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_alias_write_locked(&mut tx, site_id, &rel, true)?;
        let mut paths = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .filter(
                site_entries::path.eq(&rel).or(site_entries::path
                    .ge(&prefix_start)
                    .and(site_entries::path.lt(&prefix_end))),
            )
            .select(site_entries::path)
            .load::<String>(&mut *tx)?;
        let exact_kind = site_entries::table
            .find((site_id, rel.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *tx)
            .optional()?;
        if exact_kind == Some(database::schema::ALIAS_ENTRY_KIND) {
            paths.clear();
            paths.push(rel.clone());
        }
        if paths.is_empty() {
            return Err(StoreError::NotFound);
        }
        let path_refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            UndoKind::DeletePath,
            &format!("restore deleted {rel}"),
            &path_refs,
            self.now_millis(),
        )?;
        let removed_files = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.eq_any(&paths))
            .select((files::path, files::size))
            .load::<(String, i64)>(&mut *tx)?;
        let removed_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::path.eq_any(&paths))
            .select((allocated_entries::path, allocated_entries::size))
            .load::<(String, i64)>(&mut *tx)?;
        for (path, size) in removed_files.iter().chain(&removed_allocated) {
            adjust_aggregates_locked(&mut tx, site_id, path, -*size, -1)?;
        }
        let deleted = diesel::delete(
            site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(&paths)),
        )
        .execute(&mut *tx)?;
        diesel::delete(
            expiry_policies::table
                .filter(expiry_policies::site_id.eq(site_id))
                .filter(
                    expiry_policies::path.eq(&rel).or(expiry_policies::path
                        .ge(&prefix_start)
                        .and(expiry_policies::path.lt(&prefix_end))),
                ),
        )
        .execute(&mut *tx)?;
        diesel::update(sites::table.find(site_id))
            .set(sites::content_revision.eq(sites::content_revision + 1))
            .execute(&mut *tx)?;
        refresh_aliases_locked(&mut tx, site_id, &[AliasChange::Subtree(&rel)])?;
        refresh_expiry_for_changes_locked(&mut tx, site_id, &[&rel], self.now_millis())?;
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, self.now_millis())?;
        prune_undo_locked(&mut tx, self.now_millis())?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        let (revision, tree_hash) =
            site_revision_locked(&mut tx, name).unwrap_or((0, String::new()));
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(MutationResult {
            created: false,
            changed: true,
            replayed: false,
            files: deleted,
            revision,
            tree_hash,
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        })
    }

    fn commit_site(&self, name: &str, files: &[StagedFile]) -> Result<(), StoreError> {
        self.merge_staged(name, files, UndoKind::Put).map(|_| ())
    }

    #[cfg(test)]
    fn upsert_file(&self, name: &str, file: &StagedFile) -> Result<MutationResult, StoreError> {
        reject_reserved_path(&file.path)?;
        self.merge_staged(name, std::slice::from_ref(file), UndoKind::Put)
    }

    #[allow(clippy::large_types_passed_by_value)]
    fn publish_staged(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        self.publish_staged_entries(wanted, files, &[], options)
    }

    #[allow(clippy::large_types_passed_by_value)]
    fn publish_archive_staged(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        plan: &ArchivePlan,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let aliases = plan
            .members
            .iter()
            .filter_map(|member| match member {
                ArchiveMember::Alias {
                    path,
                    canonical_target,
                } => Some(ArchiveAlias {
                    path: path.as_str(),
                    target: canonical_target.as_str(),
                }),
                ArchiveMember::File { .. } => None,
            })
            .collect::<Vec<_>>();
        self.publish_staged_entries(wanted, files, &aliases, options)
    }

    #[allow(clippy::large_types_passed_by_value)]
    fn publish_staged_entries(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        archive_aliases: &[ArchiveAlias<'_>],
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let wanted = wanted.filter(|name| !name.is_empty());
        if let Some(name) = wanted {
            let name = parse_site_name(name)?.to_string();
            let mutation = self.merge_staged_entries_conditional(
                &name,
                files,
                archive_aliases,
                UndoKind::Put,
                options.expected_tree_hash,
                options.creation,
                options.authorization,
            )?;
            return Ok((name, mutation));
        }
        if options.expected_tree_hash.is_some() {
            return Err(StoreError::PreconditionFailed {
                revision: 0,
                tree_hash: String::new(),
            });
        }
        let files = files
            .iter()
            .filter(|file| file.path != MANIFEST_PATH)
            .collect::<Vec<_>>();
        if files.is_empty() && archive_aliases.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let fingerprint = staged_entries_fingerprint(&files, archive_aliases);
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(idempotency) = options.idempotency {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let existing_names = sites::table
            .select(sites::name)
            .load::<String>(&mut *tx)?
            .into_iter()
            .collect::<HashSet<_>>();
        let name = generate_id(|candidate| existing_names.contains(candidate));
        let mutation = self.merge_staged_locked(
            &mut tx,
            &files,
            MergeContext {
                name: &name,
                kind: UndoKind::Put,
                expected_tree_hash: None,
                now,
                creation: options.creation,
                authorization: None,
                archive_aliases,
            },
        )?;
        let published = PublishedMutation {
            name: name.clone(),
            mutation: mutation.clone(),
        };
        if let Some(idempotency) = options.idempotency {
            store_idempotency(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((name, mutation))
    }

    fn merge_staged(
        &self,
        name: &str,
        files: &[StagedFile],
        kind: UndoKind,
    ) -> Result<MutationResult, StoreError> {
        self.merge_staged_conditional(name, files, kind, None, CreationSecurity::default(), None)
    }

    #[allow(clippy::too_many_arguments)]
    fn merge_staged_conditional(
        &self,
        name: &str,
        files: &[StagedFile],
        kind: UndoKind,
        expected_tree_hash: Option<&str>,
        creation: CreationSecurity,
        authorization: Option<&ManagementToken>,
    ) -> Result<MutationResult, StoreError> {
        self.merge_staged_entries_conditional(
            name,
            files,
            &[],
            kind,
            expected_tree_hash,
            creation,
            authorization,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn merge_staged_entries_conditional(
        &self,
        name: &str,
        files: &[StagedFile],
        archive_aliases: &[ArchiveAlias<'_>],
        kind: UndoKind,
        expected_tree_hash: Option<&str>,
        creation: CreationSecurity,
        authorization: Option<&ManagementToken>,
    ) -> Result<MutationResult, StoreError> {
        let files = files
            .iter()
            .filter(|file| file.path != MANIFEST_PATH)
            .collect::<Vec<_>>();
        if files.is_empty() && archive_aliases.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let mutation = self.merge_staged_locked(
            &mut tx,
            &files,
            MergeContext {
                name,
                kind,
                expected_tree_hash,
                now,
                creation,
                authorization,
                archive_aliases,
            },
        )?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(mutation)
    }

    #[allow(clippy::large_types_passed_by_value, clippy::too_many_lines)]
    fn merge_staged_locked(
        &self,
        tx: &mut SqliteConnection,
        files: &[&StagedFile],
        context: MergeContext<'_>,
    ) -> Result<MutationResult, StoreError> {
        let MergeContext {
            name,
            kind,
            expected_tree_hash,
            now,
            creation,
            authorization,
            archive_aliases,
        } = context;
        let existed = site_exists_locked(tx, name)?;
        if existed {
            authorize_locked(tx, name, authorization)?;
        }
        if let Some(expected) = expected_tree_hash {
            let (revision, tree_hash) = if existed {
                site_revision_locked(tx, name)?
            } else {
                (0, String::new())
            };
            if expected != tree_hash {
                return Err(StoreError::PreconditionFailed {
                    revision,
                    tree_hash,
                });
            }
        }
        let existing_site_id = sites::table
            .filter(sites::name.eq(name))
            .select(sites::id)
            .first::<i64>(tx)
            .optional()?;
        if let Some(site_id) = existing_site_id {
            reject_alias_writes_locked(
                tx,
                site_id,
                files.iter().map(|file| file.path.as_str()),
                false,
            )?;
            validate_archive_alias_conflicts_locked(tx, site_id, archive_aliases)?;
        }
        let mut changed = false;
        for file in files {
            let current = if let Some(site_id) = existing_site_id {
                files::table
                    .find((site_id, file.path.as_str()))
                    .select(files::hash)
                    .first::<String>(tx)
                    .optional()?
            } else {
                None
            };
            changed |= current.as_deref() != Some(file.hash.as_str());
        }
        if let Some(site_id) = existing_site_id {
            for alias in archive_aliases {
                let current = aliases::table
                    .find((site_id, alias.path))
                    .select(aliases::canonical_target)
                    .first::<String>(tx)
                    .optional()?;
                changed |= current.as_deref() != Some(alias.target);
            }
        } else {
            changed |= !archive_aliases.is_empty();
        }
        if !changed {
            let (revision, tree_hash) = site_revision_locked(tx, name)?;
            return Ok(MutationResult {
                created: false,
                changed: false,
                replayed: false,
                files: files.len() + archive_aliases.len(),
                revision,
                tree_hash,
                undo: None,
                sanitized: sanitized_counts(files),
            });
        }
        if let Some(site_id) = existing_site_id
            && !archive_aliases.is_empty()
        {
            validate_alias_graph_with_staged(tx, site_id, files, archive_aliases)?;
        }
        for file in files {
            self.materialize(file)?;
        }
        let description = if existed {
            format!("restore previous state of {name}")
        } else {
            format!("remove newly created site {name}")
        };
        let changed_paths = files
            .iter()
            .map(|file| file.path.as_str())
            .chain(archive_aliases.iter().map(|alias| alias.path))
            .collect::<Vec<_>>();
        let undo = if existed {
            snapshot_entry_deltas(tx, name, kind, &description, &changed_paths, now)?
        } else {
            snapshot_site_with_description(tx, name, kind, &description, now)?
        };
        for file in files {
            diesel::insert_into(blobs::table)
                .values((
                    blobs::hash.eq(&file.hash),
                    blobs::bytes.eq(Vec::<u8>::new()),
                    blobs::size.eq(file.size),
                ))
                .on_conflict_do_nothing()
                .execute(tx)?;
        }
        diesel::insert_into(sites::table)
            .values(NewSite {
                name,
                updated: now,
                public_url: &self.inner.public_url,
                content_revision: 0,
                tree_hash: "",
                creator_kind: creation.creator.map(|creator| creator.kind as i64),
                creator_hash: creation.creator.map(|creator| creator.hash.to_vec()),
                claim_hash: creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                management_hash: creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                management_status: i64::from(creation.management_hash.is_some()),
            })
            .on_conflict_do_nothing()
            .execute(tx)?;
        let site_id = site_id_locked(tx, name)?;
        for file in files {
            let previous_size = files::table
                .find((site_id, file.path.as_str()))
                .select(files::size)
                .first::<i64>(tx)
                .optional()?;
            ensure_file_entry(tx, site_id, &file.path)?;
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id,
                    path: &file.path,
                    hash: &file.hash,
                    size: file.size,
                })
                .on_conflict((files::site_id, files::path))
                .do_update()
                .set((
                    files::hash.eq(excluded(files::hash)),
                    files::size.eq(excluded(files::size)),
                ))
                .execute(tx)?;
            adjust_aggregates_locked(
                tx,
                site_id,
                &file.path,
                file.size - previous_size.unwrap_or(0),
                i64::from(previous_size.is_none()),
            )?;
        }
        for alias in archive_aliases {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(alias.path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .on_conflict_do_nothing()
                .execute(tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(site_id),
                    aliases::path.eq(alias.path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(alias.target),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<String>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .on_conflict((aliases::site_id, aliases::path))
                .do_update()
                .set((
                    aliases::canonical_target.eq(excluded(aliases::canonical_target)),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<String>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .execute(tx)?;
        }
        let revision = if existed {
            sites::table
                .find(site_id)
                .select(sites::content_revision + 1)
                .first::<i64>(tx)?
        } else {
            1
        };
        diesel::update(sites::table.find(site_id))
            .set((sites::updated.eq(now), sites::content_revision.eq(revision)))
            .execute(tx)?;
        let alias_changes = files
            .iter()
            .map(|file| AliasChange::Entry(file.path.as_str()))
            .chain(
                archive_aliases
                    .iter()
                    .map(|alias| AliasChange::Alias(alias.path)),
            )
            .collect::<Vec<_>>();
        refresh_aliases_locked(tx, site_id, &alias_changes)?;
        refresh_expiry_for_changes_locked(tx, site_id, &changed_paths, now)?;
        let tree_hash = regenerate_site(tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(tx, now)?;
        Ok(MutationResult {
            created: !existed,
            changed: true,
            replayed: false,
            files: files.len() + archive_aliases.len(),
            revision: revision.cast_unsigned(),
            tree_hash,
            undo: Some(undo),
            sanitized: sanitized_counts(files),
        })
    }

    fn replay_content_request(
        &self,
        name: &str,
        options: FileMutationOptions<'_>,
        request_fingerprint: &str,
    ) -> Result<Option<AllocatedFile>, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        let replay =
            entry_mutation_request_replay(&mut tx, options.idempotency, request_fingerprint);
        drop(tx);
        drop(db);
        replay
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn commit_regular(
        &self,
        name: &str,
        path: &str,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        request_fingerprint: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
    ) -> Result<AllocatedFile, StoreError> {
        let now = self.now_millis();
        let fingerprint = entry_mutation_fingerprint(
            name,
            Some(path),
            path,
            &staged.hash,
            kind,
            options.expected_tree_hash,
        );
        let computed_request_fingerprint;
        let request_fingerprint = if let Some(request_fingerprint) = request_fingerprint {
            request_fingerprint
        } else {
            computed_request_fingerprint = content_mutation_request_fingerprint(
                name,
                path,
                &staged.hash,
                expected_content_hash,
                options.expected_tree_hash,
                kind,
            );
            &computed_request_fingerprint
        };
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) = entry_mutation_replay(&mut tx, options.idempotency, &fingerprint)? {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_alias_write_locked(&mut tx, site_id, path, false)?;
        let current = files::table
            .find((site_id, path))
            .select((files::hash, files::size))
            .first::<(String, i64)>(&mut *tx)
            .optional()?;
        let Some((current_hash, current_size)) = current else {
            return Err(StoreError::NotFound);
        };
        if expected_content_hash.is_some_and(|expected| expected != current_hash) {
            return Err(StoreError::StaleContentHash(current_hash));
        }
        if current_hash == staged.hash {
            let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
            let result = AllocatedFile {
                path: path.to_string(),
                hash: current_hash,
                size: current_size.cast_unsigned(),
                changed: false,
                replayed: false,
                mutation: Some(MutationResult {
                    created: false,
                    changed: false,
                    replayed: false,
                    files: 1,
                    revision,
                    tree_hash,
                    undo: None,
                    sanitized: staged.sanitized,
                }),
            };
            store_entry_mutation_with_request(
                &mut tx,
                options.idempotency,
                &fingerprint,
                request_fingerprint,
                &result,
                now,
            )?;
            tx.commit()?;
            return Ok(result);
        }
        self.materialize(staged)?;
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            kind,
            &format!("restore previous {path}"),
            &[path],
            now,
        )?;
        ensure_blob_locked(&mut tx, staged)?;
        diesel::update(files::table.find((site_id, path)))
            .set((files::hash.eq(&staged.hash), files::size.eq(staged.size)))
            .execute(&mut *tx)?;
        adjust_aggregates_locked(&mut tx, site_id, path, staged.size - current_size, 0)?;
        finish_entry_mutation(&mut tx, &self.inner.blob_files, site_id, &[path], now)?;
        let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
        let mutation = MutationResult {
            created: false,
            changed: true,
            replayed: false,
            files: 1,
            revision,
            tree_hash,
            undo: Some(undo),
            sanitized: staged.sanitized,
        };
        let result = AllocatedFile {
            path: path.to_string(),
            hash: staged.hash.clone(),
            size: staged.size.cast_unsigned(),
            changed: true,
            replayed: false,
            mutation: Some(mutation),
        };
        store_entry_mutation_with_request(
            &mut tx,
            options.idempotency,
            &fingerprint,
            request_fingerprint,
            &result,
            now,
        )?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    fn allocated_metadata(&self, name: &str, path: &str) -> Result<AllocatedMetadata, StoreError> {
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        allocated_metadata_locked(&mut db, site_id, path)
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_allocated(
        &self,
        name: &str,
        current_path: Option<&str>,
        destination: &AllocationDestination,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        request_fingerprint: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
    ) -> Result<AllocatedFile, StoreError> {
        let now = self.now_millis();
        let options = FileMutationOptions {
            expiry: validate_file_expiry(options.expiry)?,
            ..options
        };
        let fingerprint = allocated_entry_mutation_fingerprint(&AllocatedEntryFingerprint {
            name,
            current_path,
            destination,
            hash: &staged.hash,
            kind,
            expiry: options.expiry,
            expected_tree_hash: options.expected_tree_hash,
        });
        let computed_request_fingerprint;
        let request_fingerprint = if let Some(request_fingerprint) = request_fingerprint {
            request_fingerprint
        } else {
            computed_request_fingerprint = content_mutation_request_fingerprint(
                name,
                current_path.unwrap_or(""),
                &staged.hash,
                expected_content_hash,
                options.expected_tree_hash,
                kind,
            );
            &computed_request_fingerprint
        };
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) = entry_mutation_replay(&mut tx, options.idempotency, &fingerprint)? {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        if let Some(current_path) = current_path {
            reject_alias_write_locked(&mut tx, site_id, current_path, false)?;
        }
        reject_alias_write_locked(&mut tx, site_id, &destination.path, false)?;
        let needs_materialization = validate_allocated_commit_locked(
            &mut tx,
            site_id,
            current_path,
            destination,
            &staged.hash,
            expected_content_hash,
        )?;
        if needs_materialization {
            self.materialize(staged)?;
        }
        let result = self.commit_allocated_locked(
            &mut tx,
            name,
            current_path,
            destination,
            staged,
            expected_content_hash,
            options,
            kind,
            now,
        )?;
        store_entry_mutation_with_request(
            &mut tx,
            options.idempotency,
            &fingerprint,
            request_fingerprint,
            &result,
            now,
        )?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn commit_allocated_locked(
        &self,
        tx: &mut SqliteConnection,
        name: &str,
        current_path: Option<&str>,
        destination: &AllocationDestination,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
        now: i64,
    ) -> Result<AllocatedFile, StoreError> {
        let site_id = site_id_locked(tx, name)?;
        if let Some(current_path) = current_path {
            let current_kind = site_entries::table
                .find((site_id, current_path))
                .select(site_entries::kind)
                .first::<i64>(tx)
                .map_err(map_sql)?;
            if current_kind != database::schema::ALLOCATED_ENTRY_KIND {
                return Err(StoreError::NotFound);
            }
            let (current_hash, current_size) = allocated_entries::table
                .find((site_id, current_path))
                .select((allocated_entries::hash, allocated_entries::size))
                .first::<(String, i64)>(tx)?;
            if expected_content_hash.is_some_and(|expected| expected != current_hash) {
                return Err(StoreError::StaleContentHash(current_hash));
            }
            if current_hash == staged.hash {
                let (revision, tree_hash) = site_revision_locked(tx, name)?;
                return Ok(AllocatedFile {
                    path: current_path.to_string(),
                    hash: current_hash,
                    size: current_size.cast_unsigned(),
                    changed: false,
                    replayed: false,
                    mutation: Some(MutationResult {
                        created: false,
                        changed: false,
                        replayed: false,
                        files: 1,
                        revision,
                        tree_hash,
                        undo: None,
                        sanitized: staged.sanitized,
                    }),
                });
            }
        }
        let same_path = current_path == Some(destination.path.as_str());
        let destination_kind = if same_path {
            None
        } else {
            site_entries::table
                .find((site_id, destination.path.as_str()))
                .select(site_entries::kind)
                .first::<i64>(tx)
                .optional()?
        };
        if destination_kind
            .is_some_and(|entry_kind| entry_kind != database::schema::ALLOCATED_ENTRY_KIND)
        {
            return Err(StoreError::DestinationConflict);
        }
        let reused_size = if destination_kind.is_some() {
            let metadata = allocated_metadata_locked(tx, site_id, &destination.path)?;
            if metadata.hash != staged.hash || !allocation_metadata_matches(&metadata, destination)
            {
                return Err(StoreError::DestinationConflict);
            }
            Some(metadata.size)
        } else {
            None
        };
        if current_path.is_none()
            && let Some(size) = reused_size
        {
            if file_expiry_change_required(tx, site_id, &destination.path, options.expiry)? {
                let changed_paths = [destination.path.as_str()];
                let undo = snapshot_entry_deltas(
                    tx,
                    name,
                    kind,
                    &format!("restore previous allocated entry in {name}"),
                    &changed_paths,
                    now,
                )?;
                apply_file_expiry(
                    tx,
                    site_id,
                    &destination.path,
                    size.cast_unsigned(),
                    options.expiry,
                    now,
                )?;
                finish_entry_mutation(tx, &self.inner.blob_files, site_id, &changed_paths, now)?;
                let (revision, tree_hash) = site_revision_locked(tx, name)?;
                return Ok(AllocatedFile {
                    path: destination.path.clone(),
                    hash: staged.hash.clone(),
                    size: size.cast_unsigned(),
                    changed: true,
                    replayed: false,
                    mutation: Some(MutationResult {
                        created: false,
                        changed: true,
                        replayed: false,
                        files: 1,
                        revision,
                        tree_hash,
                        undo: Some(undo),
                        sanitized: staged.sanitized,
                    }),
                });
            }
            let (revision, tree_hash) = site_revision_locked(tx, name)?;
            return Ok(AllocatedFile {
                path: destination.path.clone(),
                hash: staged.hash.clone(),
                size: size.cast_unsigned(),
                changed: false,
                replayed: false,
                mutation: Some(MutationResult {
                    created: false,
                    changed: false,
                    replayed: false,
                    files: 1,
                    revision,
                    tree_hash,
                    undo: None,
                    sanitized: staged.sanitized,
                }),
            });
        }
        let mut changed_paths = Vec::with_capacity(2);
        if let Some(current_path) = current_path {
            changed_paths.push(current_path);
        }
        if reused_size.is_none() && current_path != Some(destination.path.as_str()) {
            changed_paths.push(destination.path.as_str());
        }
        let undo = snapshot_entry_deltas(
            tx,
            name,
            kind,
            &format!("restore previous allocated entry in {name}"),
            &changed_paths,
            now,
        )?;
        ensure_blob_locked(tx, staged)?;
        if let Some(current_path) = current_path {
            let previous_size = allocated_entries::table
                .find((site_id, current_path))
                .select(allocated_entries::size)
                .first::<i64>(tx)?;
            if reused_size.is_some() {
                diesel::delete(expiry_policies::table.find((site_id, current_path))).execute(tx)?;
            } else {
                diesel::update(expiry_policies::table.find((site_id, current_path)))
                    .set(expiry_policies::path.eq(&destination.path))
                    .execute(tx)?;
            }
            diesel::delete(site_entries::table.find((site_id, current_path))).execute(tx)?;
            adjust_aggregates_locked(tx, site_id, current_path, -previous_size, -1)?;
        }
        if reused_size.is_none() {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(&destination.path),
                    site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                ))
                .execute(tx)?;
            diesel::insert_into(allocated_entries::table)
                .values((
                    allocated_entries::site_id.eq(site_id),
                    allocated_entries::path.eq(&destination.path),
                    allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    allocated_entries::hash.eq(&staged.hash),
                    allocated_entries::size.eq(staged.size),
                    allocated_entries::naming_mode.eq(destination.naming_mode as i64),
                    allocated_entries::prefix.eq(&destination.prefix),
                    allocated_entries::suffix.eq(&destination.suffix),
                    allocated_entries::extension.eq(&destination.extension),
                    allocated_entries::media_type.eq(&destination.media_type),
                ))
                .execute(tx)?;
            adjust_aggregates_locked(tx, site_id, &destination.path, staged.size, 1)?;
        }
        apply_file_expiry(
            tx,
            site_id,
            &destination.path,
            reused_size.unwrap_or(staged.size).cast_unsigned(),
            options.expiry,
            now,
        )?;
        finish_entry_mutation(tx, &self.inner.blob_files, site_id, &changed_paths, now)?;
        let (revision, tree_hash) = site_revision_locked(tx, name)?;
        Ok(AllocatedFile {
            path: destination.path.clone(),
            hash: staged.hash.clone(),
            size: reused_size.unwrap_or(staged.size).cast_unsigned(),
            changed: true,
            replayed: false,
            mutation: Some(MutationResult {
                created: current_path.is_none(),
                changed: true,
                replayed: false,
                files: 1,
                revision,
                tree_hash,
                undo: Some(undo),
                sanitized: staged.sanitized,
            }),
        })
    }

    fn materialize(&self, file: &StagedFile) -> Result<(), StoreError> {
        match &file.source {
            StagedSource::Bytes(bytes) => self.inner.blob_files.put_bytes(&file.hash, bytes)?,
            StagedSource::File(path) | StagedSource::Temporary(path) => {
                self.inner.blob_files.put_file(&file.hash, path)?;
            }
        }
        Ok(())
    }

    fn stage_allocation_source(
        &self,
        source: AllocationSource<'_>,
        path: &str,
    ) -> Result<StagedFile, StoreError> {
        match source {
            AllocationSource::Bytes(bytes) => Ok(stage_bytes(path, bytes)),
            AllocationSource::File(source) => {
                let temporary = TemporaryDirectory::create(self.tmp_dir("source"))?;
                let copied = temporary.path().join("content");
                let mut input = fs::File::open(source)?;
                let mut output = fs::File::create(&copied)?;
                io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                let staged = stage_temporary_file(path, copied)?;
                temporary.persist();
                Ok(staged)
            }
        }
    }

    fn remove_blob_files(&self, hashes: &[String]) {
        self.inner.blobs.remove(hashes);
        for hash in hashes {
            if let Err(err) = self.inner.blob_files.quarantine(hash) {
                tracing::warn!(%hash, %err, "failed to quarantine unreferenced blob file");
            }
        }
    }

    fn restore_quarantined_blob_files(&self) -> Result<(), StoreError> {
        let mut db = self.inner.readers.get();
        let live = blobs::table
            .select(blobs::hash)
            .load::<String>(&mut *db)?
            .into_iter()
            .collect::<HashSet<_>>();
        drop(db);
        self.inner.blob_files.restore(&live)?;
        Ok(())
    }

    fn migrate_sqlite_blobs(&self) -> Result<(), StoreError> {
        let mut db = self.inner.writer.lock().unwrap();
        let migrated = metadata::table
            .find("external_blobs_v1")
            .select(metadata::key)
            .first::<String>(&mut *db)
            .optional()?
            .is_some();
        if migrated {
            return Ok(());
        }

        {
            let rows = blobs::table
                .select((blobs::hash, blobs::bytes, blobs::size))
                .order(blobs::hash)
                .load::<(String, Vec<u8>, i64)>(&mut *db)?;
            for (hash, bytes, size) in rows {
                if i64::try_from(bytes.len()).expect("blob size fits in i64") != size
                    || blake3::hash(&bytes).to_hex().as_str() != hash
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("corrupt SQLite blob {hash}"),
                    )
                    .into());
                }
                self.inner.blob_files.put_bytes(&hash, &bytes)?;
            }
        }

        let mut tx = DbTransaction::begin(&mut db)?;
        diesel::update(blobs::table)
            .set(blobs::bytes.eq(Vec::<u8>::new()))
            .execute(&mut *tx)?;
        diesel::insert_into(metadata::table)
            .values((
                metadata::key.eq("external_blobs_v1"),
                metadata::value.eq("1"),
            ))
            .execute(&mut *tx)?;
        tx.commit()?;
        if let Err(err) = db.batch_execute("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;") {
            tracing::warn!(%err, "blob migration succeeded but database compaction failed");
        }
        drop(db);
        Ok(())
    }

    fn migrate_legacy(&self) -> Result<(), StoreError> {
        {
            let mut db = self.inner.writer.lock().unwrap();
            let n = sites::table.select(count_star()).first::<i64>(&mut *db)?;
            drop(db);
            if n > 0 {
                return Ok(());
            }
        }
        let catalog = self.inner.root.join("catalog.json");
        if catalog.is_file() {
            self.migrate_json(&catalog)?;
            return Ok(());
        }
        let legacy = self.inner.root.join("sites");
        if legacy.is_dir() {
            self.migrate_tree(&legacy)?;
        }
        Ok(())
    }

    fn migrate_json(&self, path: &Path) -> Result<(), StoreError> {
        #[derive(serde::Deserialize)]
        struct DiskCatalog {
            sites: Vec<DiskSite>,
        }
        #[derive(serde::Deserialize)]
        struct DiskSite {
            name: String,
            files: Vec<DiskFile>,
        }
        #[derive(serde::Deserialize)]
        struct DiskFile {
            path: String,
            hash: String,
        }
        let parsed: DiskCatalog = serde_json::from_slice(&fs::read(path)?)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        for site in parsed.sites {
            let mut staged = Vec::new();
            for file in site.files {
                let blob = self
                    .inner
                    .root
                    .join("blobs")
                    .join(&file.hash[..2])
                    .join(&file.hash[2..]);
                let bytes = fs::read(blob)?;
                staged.push(stage_bytes(&file.path, &bytes));
            }
            self.commit_site(&site.name, &staged)?;
        }
        Ok(())
    }

    fn migrate_tree(&self, dir: &Path) -> Result<(), StoreError> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if parse_site_name(&name).is_err() {
                continue;
            }
            let staged = stage_dir(&entry.path())?;
            self.commit_site(&name, &staged)?;
        }
        Ok(())
    }

    pub fn undo_stack(&self, name: &str) -> Result<UndoStack, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.readers.get();
        let rows = undo_operations::table
            .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
            .filter(undo_names::name.eq(name))
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .select((
                undo_operations::token,
                undo_operations::kind,
                undo_operations::description,
                undo_operations::created,
                undo_operations::expires,
            ))
            .order((
                undo_operations::created.desc(),
                undo_operations::rowid.desc(),
            ))
            .load::<(String, i64, String, i64, i64)>(&mut *db)?;
        let entries = rows
            .into_iter()
            .map(|(token, kind, description, created, expires)| {
                Ok(UndoEntry {
                    token,
                    kind: UndoKind::from_i64(kind)?.as_str().to_string(),
                    description,
                    created_at: format_timestamp(created),
                    expires_at: format_timestamp(expires),
                    remaining_seconds: u64::try_from((expires - now).max(0) / 1000)
                        .expect("remaining time is non-negative"),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(UndoStack {
            site: name.to_string(),
            entries,
        })
    }

    #[cfg(test)]
    pub fn undo(&self, name: &str, guard: Option<&str>) -> Result<UndoResult, StoreError> {
        self.undo_secured(name, guard, None)
    }

    #[allow(clippy::too_many_lines)]
    pub fn undo_secured(
        &self,
        name: &str,
        guard: Option<&str>,
        authorization: Option<&ManagementToken>,
    ) -> Result<UndoResult, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let latest = undo_operations::table
            .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
            .filter(undo_names::name.eq(name))
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .select(undo_operations::token)
            .order((
                undo_operations::created.desc(),
                undo_operations::rowid.desc(),
            ))
            .first::<String>(&mut *tx)
            .optional()?
            .ok_or(StoreError::NotFound)?;
        if guard.is_some_and(|token| token != latest) {
            return Err(StoreError::StaleUndo(latest));
        }
        let latest_kind = UndoKind::from_i64(
            undo_operations::table
                .find(&latest)
                .select(undo_operations::kind)
                .first::<i64>(&mut *tx)?,
        )?;
        let has_deltas = matches!(
            latest_kind,
            UndoKind::Put
                | UndoKind::PutFile
                | UndoKind::DeletePath
                | UndoKind::Allocate
                | UndoKind::Replace
                | UndoKind::Splice
                | UndoKind::Alias
        ) && (undo_file_deltas::table
            .filter(undo_file_deltas::token.eq(&latest))
            .select(count_star())
            .first::<i64>(&mut *tx)?
            + undo_allocated_deltas::table
                .filter(undo_allocated_deltas::token.eq(&latest))
                .select(count_star())
                .first::<i64>(&mut *tx)?
            + undo_alias_deltas::table
                .filter(undo_alias_deltas::token.eq(&latest))
                .select(count_star())
                .first::<i64>(&mut *tx)?
            > 0);
        if has_deltas {
            let restored = restore_entry_deltas(&mut tx, &self.inner.blob_files, name, &latest)?;
            diesel::update(undo_operations::table.find(&latest))
                .set(undo_operations::consumed.eq(1_i64))
                .execute(&mut *tx)?;
            prune_undo_locked(&mut tx, now)?;
            let removed = gc_blobs(&mut tx, now)?;
            tx.commit()?;
            drop(db);
            self.remove_blob_files(&removed);
            return Ok(UndoResult {
                restored_at: format_timestamp(restored),
            });
        }
        let (snapshot_name, existed, public_url, updated, content_revision, tree_hash) =
            undo_sites::table
                .find(&latest)
                .select((
                    undo_sites::name,
                    undo_sites::existed,
                    undo_sites::public_url,
                    undo_sites::updated,
                    undo_sites::content_revision,
                    undo_sites::tree_hash,
                ))
                .first::<(String, i64, String, i64, i64, String)>(&mut *tx)?;
        let snapshot = SiteSnapshot {
            name: snapshot_name,
            existed: existed != 0,
            public_url,
            updated,
            content_revision,
            tree_hash,
        };
        let names = undo_names::table
            .filter(undo_names::token.eq(&latest))
            .select(undo_names::name)
            .load::<String>(&mut *tx)?;
        for name in names {
            retain_management_tombstone(&mut tx, &name, now)?;
            diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        }
        if snapshot.existed {
            let site_id = diesel::insert_into(sites::table)
                .values(NewSite {
                    name: &snapshot.name,
                    updated: snapshot.updated,
                    public_url: &snapshot.public_url,
                    content_revision: snapshot.content_revision,
                    tree_hash: &snapshot.tree_hash,
                    creator_kind: None,
                    creator_hash: None,
                    claim_hash: None,
                    management_hash: None,
                    management_status: 0,
                })
                .returning(sites::id)
                .get_result::<i64>(&mut *tx)?;
            let retained_hash = management_tombstones::table
                .inner_join(undo_names::table.on(undo_names::name.eq(management_tombstones::name)))
                .filter(undo_names::token.eq(&latest))
                .select(management_tombstones::management_hash)
                .first::<Vec<u8>>(&mut *tx)
                .optional()?;
            if let Some(hash) = retained_hash {
                diesel::update(sites::table.find(site_id))
                    .set((
                        sites::management_hash.eq(Some(hash)),
                        sites::management_status.eq(1_i64),
                    ))
                    .execute(&mut *tx)?;
            }
            let saved_files = undo_files::table
                .filter(undo_files::token.eq(&latest))
                .select((undo_files::path, undo_files::hash, undo_files::size))
                .load::<(String, String, i64)>(&mut *tx)?;
            for (path, hash, size) in saved_files {
                ensure_file_entry(&mut tx, site_id, &path)?;
                diesel::insert_into(files::table)
                    .values(NewFile {
                        site_id,
                        path: &path,
                        hash: &hash,
                        size,
                    })
                    .execute(&mut *tx)?;
            }
            let saved_allocated = undo_allocated_deltas::table
                .filter(undo_allocated_deltas::token.eq(&latest))
                .filter(undo_allocated_deltas::existed.eq(1_i64))
                .select((
                    undo_allocated_deltas::path,
                    undo_allocated_deltas::hash,
                    undo_allocated_deltas::size,
                    undo_allocated_deltas::naming_mode,
                    undo_allocated_deltas::prefix,
                    undo_allocated_deltas::suffix,
                    undo_allocated_deltas::extension,
                    undo_allocated_deltas::media_type,
                ))
                .load::<(
                    String,
                    Option<String>,
                    Option<i64>,
                    Option<i64>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                )>(&mut *tx)?;
            for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
                saved_allocated
            {
                diesel::insert_into(site_entries::table)
                    .values((
                        site_entries::site_id.eq(site_id),
                        site_entries::path.eq(&path),
                        site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    ))
                    .execute(&mut *tx)?;
                diesel::insert_into(allocated_entries::table)
                    .values(
                        (
                            allocated_entries::site_id.eq(site_id),
                            allocated_entries::path.eq(path),
                            allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                            allocated_entries::hash
                                .eq(hash.expect("whole-site allocated snapshot has hash")),
                            allocated_entries::size
                                .eq(size.expect("whole-site allocated snapshot has size")),
                            allocated_entries::naming_mode
                                .eq(naming_mode
                                    .expect("whole-site allocated snapshot has naming mode")),
                            allocated_entries::prefix
                                .eq(prefix.expect("whole-site allocated snapshot has prefix")),
                            allocated_entries::suffix
                                .eq(suffix.expect("whole-site allocated snapshot has suffix")),
                            allocated_entries::extension.eq(extension),
                            allocated_entries::media_type
                                .eq(media_type
                                    .expect("whole-site allocated snapshot has media type")),
                        ),
                    )
                    .execute(&mut *tx)?;
            }
            let saved_aliases = undo_alias_deltas::table
                .filter(undo_alias_deltas::token.eq(&latest))
                .filter(undo_alias_deltas::existed.eq(1_i64))
                .select(UndoAliasRow::as_select())
                .load::<UndoAliasRow>(&mut *tx)?;
            for alias in saved_aliases {
                diesel::insert_into(site_entries::table)
                    .values((
                        site_entries::site_id.eq(site_id),
                        site_entries::path.eq(&alias.path),
                        site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    ))
                    .execute(&mut *tx)?;
                diesel::insert_into(aliases::table)
                    .values((
                        aliases::site_id.eq(site_id),
                        aliases::path.eq(alias.path),
                        aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                        aliases::canonical_target.eq(alias
                            .canonical_target
                            .expect("whole-site alias snapshot has target")),
                        aliases::resolved_kind.eq(alias.resolved_kind),
                        aliases::resolved_hash.eq(alias.resolved_hash),
                        aliases::resolved_size.eq(alias.resolved_size),
                    ))
                    .execute(&mut *tx)?;
            }
            restore_expiry_policies_locked(&mut tx, &latest, site_id)?;
            rebuild_aggregates_locked(&mut tx, site_id)?;
            regenerate_site(&mut tx, &self.inner.blob_files, site_id, snapshot.updated)?;
            let tombstone_names = undo_names::table
                .filter(undo_names::token.eq(&latest))
                .select(undo_names::name)
                .load::<String>(&mut *tx)?;
            for names in tombstone_names.chunks(SQLITE_DELETE_BATCH_SIZE) {
                diesel::delete(
                    management_tombstones::table.filter(management_tombstones::name.eq_any(names)),
                )
                .execute(&mut *tx)?;
            }
        }
        diesel::update(undo_operations::table.find(&latest))
            .set(undo_operations::consumed.eq(1_i64))
            .execute(&mut *tx)?;
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(UndoResult {
            restored_at: format_timestamp(snapshot.updated),
        })
    }

    #[cfg(test)]
    pub fn set_expiry(
        &self,
        name: &str,
        rel: &str,
        policy: Option<ExpiryPolicy>,
    ) -> Result<ExpiryMutation, StoreError> {
        self.set_expiry_secured(name, rel, policy, None)
    }

    pub fn set_expiry_secured(
        &self,
        name: &str,
        rel: &str,
        policy: Option<ExpiryPolicy>,
        authorization: Option<&ManagementToken>,
    ) -> Result<ExpiryMutation, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let policy = policy.map(ExpiryPolicy::validate).transpose()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_expiry_below_alias_locked(&mut tx, site_id, &rel)?;
        let kind = expiry_target_kind_locked(&mut tx, name, &rel)?;
        let previous = expiry_policies::table
            .find((site_id, rel.as_str()))
            .select(expiry_policies::site_id)
            .first::<i64>(&mut *tx)
            .optional()?
            .is_some();
        let undo = if previous || policy.is_some() {
            Some(snapshot_site_with_description(
                &mut tx,
                name,
                UndoKind::Expiry,
                &format!(
                    "restore previous expiry policy for {}",
                    expiry_display_path(name, &rel)
                ),
                now,
            )?)
        } else {
            None
        };
        if let Some(policy) = policy {
            let size = expiry_target_size_locked(&mut tx, site_id, &rel, kind)?;
            store_expiry_policy_locked(
                &mut tx,
                ExpiryPolicyWrite {
                    site_id,
                    path: &rel,
                    kind,
                    policy,
                    size,
                    now,
                },
            )?;
        } else {
            diesel::delete(expiry_policies::table.find((site_id, rel.as_str())))
                .execute(&mut *tx)?;
        }
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        let report = self.expiry_report(name, &rel)?;
        Ok(ExpiryMutation { report, undo })
    }

    pub fn set_default_expiry_secured(
        &self,
        name: &str,
        rel: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<ExpiryMutation, StoreError> {
        self.set_expiry_secured(
            name,
            rel,
            Some(ExpiryPolicy::Decay(self.inner.expiry_defaults)),
            authorization,
        )
    }

    pub fn expiry_site_report(&self, name: &str) -> Result<ExpirySiteReport, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        let paths = expiry_policies::table
            .filter(expiry_policies::site_id.eq(site_id))
            .select(expiry_policies::path)
            .order(expiry_policies::path)
            .load::<String>(&mut *db)?;
        drop(db);
        let entries = paths
            .into_iter()
            .map(|path| self.expiry_report(name, &path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExpirySiteReport {
            site: name.to_string(),
            entries,
        })
    }

    pub fn expiry_report(&self, name: &str, rel: &str) -> Result<ExpiryReport, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let now = self.now_millis();
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        expiry_report_locked(&mut db, name, site_id, &rel, now)
    }

    pub fn next_expiry_delay(&self) -> Result<std::time::Duration, StoreError> {
        let mut db = self.inner.readers.get();
        let next = expiry_policies::table
            .select(min(expiry_policies::own_deadline))
            .first::<Option<i64>>(&mut *db)?;
        let millis = next.map_or(60_000, |deadline| {
            deadline.saturating_sub(self.now_millis()).clamp(0, 60_000)
        });
        Ok(std::time::Duration::from_millis(
            u64::try_from(millis).expect("delay is non-negative"),
        ))
    }

    #[allow(clippy::too_many_lines)]
    pub fn sweep_expired(&self) -> Result<usize, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let due = expiry_policies::table
            .inner_join(sites::table)
            .filter(expiry_policies::own_deadline.le(now))
            .select((
                sites::name,
                expiry_policies::path,
                expiry_policies::target_kind,
            ))
            .order((
                sites::id,
                expiry_policies::target_kind,
                expiry_policies::path,
            ))
            .load::<(String, String, i64)>(&mut *tx)?;
        let mut swept_sites = HashSet::new();
        let mut removed_targets = 0;
        for (name, path, raw_kind) in due {
            if !site_exists_locked(&mut tx, &name)? {
                continue;
            }
            let kind = ExpiryTargetKind::try_from(raw_kind)?;
            let site_id = site_id_locked(&mut tx, &name)?;
            let deadline = expiry_policies::table
                .find((site_id, path.as_str()))
                .select(expiry_policies::own_deadline)
                .first::<Option<i64>>(&mut *tx)
                .optional()?
                .flatten();
            let still_due = deadline.is_some_and(|deadline| deadline <= now);
            if !still_due {
                continue;
            }
            if swept_sites.insert(name.clone()) {
                snapshot_site_with_description(
                    &mut tx,
                    &name,
                    UndoKind::ExpireSweep,
                    &format!("restore expired content in {name}"),
                    now,
                )?;
            }
            let exact_alias = !path.is_empty()
                && site_entries::table
                    .find((site_id, path.as_str()))
                    .select(site_entries::kind)
                    .first::<i64>(&mut *tx)
                    .optional()?
                    == Some(database::schema::ALIAS_ENTRY_KIND);
            if exact_alias {
                diesel::delete(site_entries::table.find((site_id, path.as_str())))
                    .execute(&mut *tx)?;
                diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                    .execute(&mut *tx)?;
                finish_partial_expiry_locked(&mut tx, &self.inner.blob_files, site_id, &path, now)?;
                removed_targets += 1;
                continue;
            }
            match kind {
                ExpiryTargetKind::Site => {
                    retain_management_tombstone(&mut tx, &name, now)?;
                    diesel::delete(sites::table.find(site_id)).execute(&mut *tx)?;
                }
                ExpiryTargetKind::File => {
                    let entry_kind = site_entries::table
                        .find((site_id, path.as_str()))
                        .select(site_entries::kind)
                        .first::<i64>(&mut *tx)?;
                    let size =
                        i64::try_from(entry_size_locked(&mut tx, site_id, &path, entry_kind)?)
                            .expect("stored size fits in i64");
                    diesel::delete(site_entries::table.find((site_id, path.as_str())))
                        .execute(&mut *tx)?;
                    adjust_aggregates_locked(&mut tx, site_id, &path, -size, -1)?;
                    diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                        .execute(&mut *tx)?;
                    finish_partial_expiry_locked(
                        &mut tx,
                        &self.inner.blob_files,
                        site_id,
                        &path,
                        now,
                    )?;
                }
                ExpiryTargetKind::Folder => {
                    let alias_folder = site_entries::table
                        .find((site_id, path.as_str()))
                        .select(site_entries::kind)
                        .first::<i64>(&mut *tx)
                        .optional()?
                        == Some(database::schema::ALIAS_ENTRY_KIND);
                    if alias_folder {
                        diesel::delete(site_entries::table.find((site_id, path.as_str())))
                            .execute(&mut *tx)?;
                        diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                            .execute(&mut *tx)?;
                    } else {
                        let (start, end) = descendant_bounds(&path);
                        let removed_files = files::table
                            .filter(files::site_id.eq(site_id))
                            .filter(files::path.ge(&start))
                            .filter(files::path.lt(&end))
                            .select((files::path, files::size))
                            .load::<(String, i64)>(&mut *tx)?;
                        let removed_allocated = allocated_entries::table
                            .filter(allocated_entries::site_id.eq(site_id))
                            .filter(allocated_entries::path.ge(&start))
                            .filter(allocated_entries::path.lt(&end))
                            .select((allocated_entries::path, allocated_entries::size))
                            .load::<(String, i64)>(&mut *tx)?;
                        diesel::delete(
                            site_entries::table
                                .filter(site_entries::site_id.eq(site_id))
                                .filter(site_entries::path.ge(&start))
                                .filter(site_entries::path.lt(&end)),
                        )
                        .execute(&mut *tx)?;
                        for (removed_path, size) in
                            removed_files.into_iter().chain(removed_allocated)
                        {
                            adjust_aggregates_locked(&mut tx, site_id, &removed_path, -size, -1)?;
                        }
                        diesel::delete(
                            expiry_policies::table
                                .filter(expiry_policies::site_id.eq(site_id))
                                .filter(
                                    expiry_policies::path.eq(&path).or(expiry_policies::path
                                        .ge(&start)
                                        .and(expiry_policies::path.lt(&end))),
                                ),
                        )
                        .execute(&mut *tx)?;
                    }
                    finish_partial_expiry_locked(
                        &mut tx,
                        &self.inner.blob_files,
                        site_id,
                        &path,
                        now,
                    )?;
                }
            }
            removed_targets += 1;
        }
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(removed_targets)
    }

    fn backfill_manifests(&self) -> Result<(), StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let ids = sites::table
            .select(sites::id)
            .order(sites::id)
            .load::<i64>(&mut *tx)?;
        for site_id in ids {
            let revision = sites::table
                .find(site_id)
                .select(sites::content_revision)
                .first::<i64>(&mut *tx)?;
            diesel::update(sites::table.find(site_id))
                .set((
                    sites::public_url.eq(&self.inner.public_url),
                    sites::content_revision.eq(if revision == 0 { 1 } else { revision }),
                ))
                .execute(&mut *tx)?;
            rebuild_aggregates_locked(&mut tx, site_id)?;
            refresh_all_aliases_locked(&mut tx, site_id)?;
            regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        }
        tx.commit()?;
        drop(db);
        Ok(())
    }

    fn prune_undo_and_gc(&self) -> Result<(), StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_undo_locked(&mut tx, now)?;
        prune_idempotency_locked(&mut tx, now)?;
        prune_pending_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }

    fn now_millis(&self) -> i64 {
        self.inner.clock.now_millis()
    }

    #[cfg(test)]
    fn set_before_content_commit(&self, hook: impl FnOnce() + Send + 'static) {
        *self.inner.before_content_commit.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn run_before_content_commit(&self) {
        let hook = self.inner.before_content_commit.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(not(test))]
    #[allow(clippy::missing_const_for_fn, clippy::unused_self)]
    fn run_before_content_commit(&self) {}

    fn tmp_dir(&self, name: &str) -> PathBuf {
        let t = self.now_millis();
        let generation = self.inner.temp_generation.fetch_add(1, Ordering::Relaxed);
        self.inner
            .root
            .join("tmp")
            .join(format!("{name}-{}-{t}-{generation}", std::process::id()))
    }
}

#[derive(Clone, Copy)]
#[repr(i64)]
enum UndoKind {
    Put = 1,
    DeletePath = 2,
    DeleteSite = 3,
    Copy = 4,
    Move = 5,
    Expiry = 6,
    ExpireSweep = 7,
    PutFile = 8,
    Allocate = 9,
    Replace = 10,
    Splice = 11,
    Alias = 12,
}

impl UndoKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::DeletePath => "delete_path",
            Self::DeleteSite => "delete_site",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Expiry => "expiry",
            Self::ExpireSweep => "expire_sweep",
            Self::PutFile => "put_file",
            Self::Allocate => "allocate",
            Self::Replace => "replace",
            Self::Splice => "splice",
            Self::Alias => "alias",
        }
    }

    const fn from_i64(value: i64) -> Result<Self, StoreError> {
        match value {
            1 => Ok(Self::Put),
            2 => Ok(Self::DeletePath),
            3 => Ok(Self::DeleteSite),
            4 => Ok(Self::Copy),
            5 => Ok(Self::Move),
            6 => Ok(Self::Expiry),
            7 => Ok(Self::ExpireSweep),
            8 => Ok(Self::PutFile),
            9 => Ok(Self::Allocate),
            10 => Ok(Self::Replace),
            11 => Ok(Self::Splice),
            12 => Ok(Self::Alias),
            _ => Err(StoreError::UnsupportedUndoKind(value)),
        }
    }
}

#[derive(Clone, Copy)]
#[repr(i64)]
enum IdempotencyKind {
    UnnamedPut = 1,
    AutoCopy = 2,
    EntryMutation = 3,
    AliasMutation = 4,
    PendingAllocation = 5,
    AllocationCancellation = 6,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PublishedMutation {
    name: String,
    mutation: MutationResult,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct EntryMutationRecord {
    result: AllocatedFile,
    #[serde(default)]
    request_fingerprint: String,
}

#[derive(Clone, Copy)]
struct MergeContext<'a> {
    name: &'a str,
    kind: UndoKind,
    expected_tree_hash: Option<&'a str>,
    now: i64,
    creation: CreationSecurity,
    authorization: Option<&'a ManagementToken>,
    archive_aliases: &'a [ArchiveAlias<'a>],
}

#[derive(Clone, Copy)]
struct ArchiveAlias<'a> {
    path: &'a str,
    target: &'a str,
}

struct SiteSnapshot {
    name: String,
    existed: bool,
    public_url: String,
    updated: i64,
    content_revision: i64,
    tree_hash: String,
}

#[derive(Clone, Copy)]
struct StoredExpiryPolicy {
    kind: ExpiryTargetKind,
    policy: ExpiryPolicy,
    refreshed_millis: Option<i64>,
    own_deadline_millis: i64,
    size_bytes: u64,
}

#[derive(Clone, Copy)]
struct ExpiryPolicyWrite<'a> {
    site_id: i64,
    path: &'a str,
    kind: ExpiryTargetKind,
    policy: ExpiryPolicy,
    size: u64,
    now: i64,
}

fn run_migrations(db: &mut SqliteConnection) -> Result<(), StoreError> {
    let outcome = database::migrations::migrate(db)
        .map_err(|error| StoreError::Migration(Box::new(error)))?;
    if outcome.upgraded_from_v2 {
        db.transaction::<_, StoreError, _>(|connection| {
            let site_ids = sites::table.select(sites::id).load::<i64>(connection)?;
            for site_id in site_ids {
                rebuild_aggregates_locked(connection, site_id)?;
            }
            Ok(())
        })?;
    }
    let integrity = diesel::sql_query("PRAGMA integrity_check")
        .get_result::<IntegrityCheck>(db)?
        .integrity_check;
    if integrity != "ok" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, integrity).into());
    }
    let foreign_key_violations =
        diesel::sql_query("SELECT COUNT(*) AS violation_count FROM pragma_foreign_key_check")
            .get_result::<ForeignKeyViolationCount>(db)?
            .violation_count;
    if foreign_key_violations != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{foreign_key_violations} foreign-key violations"),
        )
        .into());
    }
    Ok(())
}

fn allocation_destination(
    hash: &str,
    spec: AllocationSpec<'_>,
) -> Result<AllocationDestination, StoreError> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !safe_name_fragment(spec.naming.prefix)
        || !safe_name_fragment(spec.naming.suffix)
    {
        return Err(StoreError::InvalidAllocatedName);
    }
    let folder = normalize_folder(spec.folder)?;
    let extension = spec.naming.extension.map(normalize_extension).transpose()?;
    let media_type = normalize_media_type(spec.media_type)?;
    let mut basename = String::with_capacity(
        spec.naming.prefix.len()
            + hash.len()
            + spec.naming.suffix.len()
            + extension.as_ref().map_or(0, |value| value.len() + 1),
    );
    basename.push_str(spec.naming.prefix);
    basename.push_str(hash);
    basename.push_str(spec.naming.suffix);
    if let Some(extension) = &extension {
        basename.push('.');
        basename.push_str(extension);
    }
    let path = join_folder(&folder, &basename);
    safe_rel_path(&path).map_err(|_| StoreError::InvalidAllocatedName)?;
    reject_reserved_path(&path)?;
    Ok(AllocationDestination {
        path,
        naming_mode: AllocatedNamingMode::ContentAddressed,
        prefix: spec.naming.prefix.to_string(),
        suffix: spec.naming.suffix.to_string(),
        extension,
        media_type,
    })
}

fn custom_destination(
    folder: &str,
    basename: &str,
    media_type: &str,
) -> Result<AllocationDestination, StoreError> {
    let folder = normalize_folder(folder)?;
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.contains(['/', '\\'])
        || !safe_name_fragment(basename)
        || is_reserved_path(basename)
    {
        return Err(StoreError::InvalidAllocatedName);
    }
    safe_rel_path(basename).map_err(|_| StoreError::InvalidAllocatedName)?;
    Ok(AllocationDestination {
        path: join_folder(&folder, basename),
        naming_mode: AllocatedNamingMode::Custom,
        prefix: basename.to_string(),
        suffix: String::new(),
        extension: None,
        media_type: normalize_media_type(media_type)?,
    })
}

fn relocated_destination(
    hash: &str,
    current_path: &str,
    metadata: &AllocatedMetadata,
) -> Result<AllocationDestination, StoreError> {
    let folder = current_path
        .rsplit_once('/')
        .map_or("", |(folder, _)| folder);
    match metadata.naming_mode {
        AllocatedNamingMode::ContentAddressed => allocation_destination(
            hash,
            AllocationSpec {
                folder,
                naming: AllocatedName {
                    prefix: &metadata.prefix,
                    suffix: &metadata.suffix,
                    extension: metadata.extension.as_deref(),
                },
                media_type: &metadata.media_type,
            },
        ),
        AllocatedNamingMode::Custom => {
            let basename = current_path.rsplit('/').next().unwrap_or(current_path);
            custom_destination(folder, basename, &metadata.media_type)
        }
    }
}

fn normalize_folder(folder: &str) -> Result<String, StoreError> {
    if folder.is_empty() {
        Ok(String::new())
    } else {
        normalize_rel(folder)
    }
}

fn join_folder(folder: &str, basename: &str) -> String {
    if folder.is_empty() {
        basename.to_string()
    } else {
        format!("{folder}/{basename}")
    }
}

fn normalize_media_type(media_type: &str) -> Result<String, StoreError> {
    let media_type = media_type.trim();
    media_type
        .parse::<mime_guess::mime::Mime>()
        .map(|media_type| media_type.to_string())
        .map_err(|_| StoreError::InvalidAllocatedName)
}

fn pending_fingerprint(input: &PendingFingerprint<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-allocation-v1\0");
    for value in [input.site, input.folder, input.hash, input.media_type] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&input.content_size.to_le_bytes());
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.authorization_hash.unwrap_or("").as_bytes());
    for value in [
        input.extension.unwrap_or(""),
        input.expected_tree_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn parse_pending_request_metadata(value: &str) -> Result<PendingRequestMetadata, StoreError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Ok(PendingRequestMetadata {
            request_fingerprint: value.to_string(),
            expiry: FileExpiry::Preserve,
            authorization_hash: None,
            extension: None,
            expected_tree_hash: None,
            legacy_fingerprint: true,
            sanitized: TokenCounts::default(),
        });
    }
    serde_json::from_str(value)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

fn legacy_pending_fingerprint(
    site: &str,
    folder: &str,
    hash: &str,
    content_size: u64,
    media_type: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-allocation-v1\0");
    for value in [site, folder, hash, media_type] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&content_size.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn validate_file_expiry(expiry: FileExpiry) -> Result<FileExpiry, StoreError> {
    match expiry {
        FileExpiry::Policy(policy) => policy
            .validate()
            .map(FileExpiry::Policy)
            .map_err(Into::into),
        FileExpiry::Preserve | FileExpiry::Clear => Ok(expiry),
    }
}

fn hash_file_expiry(hasher: &mut blake3::Hasher, expiry: FileExpiry) {
    match expiry {
        FileExpiry::Preserve => {
            hasher.update(&[0]);
        }
        FileExpiry::Clear => {
            hasher.update(&[1]);
        }
        FileExpiry::Policy(ExpiryPolicy::Relative { duration_seconds }) => {
            hasher.update(&[2]);
            hasher.update(&duration_seconds.to_le_bytes());
        }
        FileExpiry::Policy(ExpiryPolicy::Absolute {
            deadline_unix_seconds,
        }) => {
            hasher.update(&[3]);
            hasher.update(&deadline_unix_seconds.to_le_bytes());
        }
        FileExpiry::Policy(ExpiryPolicy::Decay(policy)) => {
            hasher.update(&[4]);
            hasher.update(&policy.min_age_seconds.to_le_bytes());
            hasher.update(&policy.max_age_seconds.to_le_bytes());
            hasher.update(&policy.max_size_bytes.to_le_bytes());
            hasher.update(&policy.power.to_bits().to_le_bytes());
        }
    }
}

fn authorization_fingerprint(token: &ManagementToken) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in token.hash().as_bytes() {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn cancellation_fingerprint(
    site: &str,
    token: &str,
    folder: &str,
    expected_tree_hash: Option<&str>,
    authorization_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-allocation-cancellation-v1\0");
    for value in [
        site,
        token,
        folder,
        expected_tree_hash.unwrap_or(""),
        authorization_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn pending_finalize_fingerprint(input: &PendingFinalizeFingerprint<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-finalize-v1\0");
    hasher.update(input.site.as_bytes());
    hasher.update(&[0]);
    hasher.update(input.token.as_bytes());
    hasher.update(&[0]);
    hasher.update(input.folder.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    match input.final_name {
        PendingFinalName::Generated(naming) => {
            hasher.update(&[1]);
            for value in [naming.prefix, naming.suffix, naming.extension.unwrap_or("")] {
                hasher.update(&(value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            }
        }
        PendingFinalName::Custom(basename) => {
            hasher.update(&[2]);
            hasher.update(basename.as_bytes());
        }
    }
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.authorization_hash.unwrap_or("").as_bytes());
    hasher.update(input.expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn allocated_metadata_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<AllocatedMetadata, StoreError> {
    let (hash, size, naming_mode, prefix, suffix, extension, media_type) = allocated_entries::table
        .find((site_id, path))
        .select((
            allocated_entries::hash,
            allocated_entries::size,
            allocated_entries::naming_mode,
            allocated_entries::prefix,
            allocated_entries::suffix,
            allocated_entries::extension,
            allocated_entries::media_type,
        ))
        .first::<(String, i64, i64, String, String, Option<String>, String)>(db)
        .map_err(map_sql)?;
    Ok(AllocatedMetadata {
        hash,
        size,
        naming_mode: AllocatedNamingMode::try_from(naming_mode)?,
        prefix,
        suffix,
        extension,
        media_type,
    })
}

fn allocation_metadata_matches(
    metadata: &AllocatedMetadata,
    destination: &AllocationDestination,
) -> bool {
    metadata.naming_mode == destination.naming_mode
        && metadata.prefix == destination.prefix
        && metadata.suffix == destination.suffix
        && metadata.extension == destination.extension
        && metadata.media_type == destination.media_type
}

fn validate_allocated_commit_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    current_path: Option<&str>,
    destination: &AllocationDestination,
    staged_hash: &str,
    expected_content_hash: Option<&str>,
) -> Result<bool, StoreError> {
    if let Some(current_path) = current_path {
        let current_kind = site_entries::table
            .find((site_id, current_path))
            .select(site_entries::kind)
            .first::<i64>(tx)
            .map_err(map_sql)?;
        if current_kind != database::schema::ALLOCATED_ENTRY_KIND {
            return Err(StoreError::NotFound);
        }
        let current_hash = allocated_entries::table
            .find((site_id, current_path))
            .select(allocated_entries::hash)
            .first::<String>(tx)?;
        if expected_content_hash.is_some_and(|expected| expected != current_hash) {
            return Err(StoreError::StaleContentHash(current_hash));
        }
        if current_hash == staged_hash {
            return Ok(false);
        }
    }
    if current_path == Some(destination.path.as_str()) {
        return Ok(true);
    }
    let destination_kind = site_entries::table
        .find((site_id, destination.path.as_str()))
        .select(site_entries::kind)
        .first::<i64>(tx)
        .optional()?;
    let Some(destination_kind) = destination_kind else {
        return Ok(true);
    };
    if destination_kind != database::schema::ALLOCATED_ENTRY_KIND {
        return Err(StoreError::DestinationConflict);
    }
    let metadata = allocated_metadata_locked(tx, site_id, &destination.path)?;
    if metadata.hash != staged_hash || !allocation_metadata_matches(&metadata, destination) {
        return Err(StoreError::DestinationConflict);
    }
    Ok(false)
}

fn safe_name_fragment(fragment: &str) -> bool {
    fragment.bytes().all(|byte| {
        byte.is_ascii_graphic() && byte != b'/' && byte != b'\\' && byte != b'?' && byte != b'#'
    })
}

fn normalize_extension(extension: &str) -> Result<String, StoreError> {
    let mut normalized = String::new();
    let mut separator = false;
    for character in extension.trim_matches('.').chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if normalized.is_empty() {
        Err(StoreError::InvalidAllocatedName)
    } else {
        Ok(normalized)
    }
}

pub fn normalize_allocated_extension(extension: &str) -> Result<String, StoreError> {
    normalize_extension(extension)
}

pub fn validate_mutation_target(path: &str) -> Result<(), StoreError> {
    let path = normalize_folder(path)?;
    reject_reserved_path(&path)
}

fn ensure_blob_locked(
    tx: &mut SqliteConnection,
    staged: &StagedFile,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(blobs::table)
        .values((
            blobs::hash.eq(&staged.hash),
            blobs::bytes.eq(Vec::<u8>::new()),
            blobs::size.eq(staged.size),
        ))
        .on_conflict_do_nothing()
        .execute(tx)?;
    Ok(())
}

fn check_tree_precondition(
    tx: &mut SqliteConnection,
    name: &str,
    expected: Option<&str>,
) -> Result<(), StoreError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let (revision, tree_hash) = site_revision_locked(tx, name)?;
    if expected == tree_hash {
        Ok(())
    } else {
        Err(StoreError::PreconditionFailed {
            revision,
            tree_hash,
        })
    }
}

fn entry_hash_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    kind: i64,
) -> Result<String, StoreError> {
    if kind == database::schema::FILE_ENTRY_KIND {
        files::table
            .find((site_id, path))
            .select(files::hash)
            .first::<String>(db)
            .map_err(map_sql)
    } else if kind == database::schema::ALLOCATED_ENTRY_KIND {
        allocated_entries::table
            .find((site_id, path))
            .select(allocated_entries::hash)
            .first::<String>(db)
            .map_err(map_sql)
    } else {
        Err(StoreError::NotFound)
    }
}

fn entry_size_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    kind: i64,
) -> Result<u64, StoreError> {
    let size = if kind == database::schema::FILE_ENTRY_KIND {
        files::table
            .find((site_id, path))
            .select(files::size)
            .first::<i64>(db)?
    } else if kind == database::schema::ALLOCATED_ENTRY_KIND {
        allocated_entries::table
            .find((site_id, path))
            .select(allocated_entries::size)
            .first::<i64>(db)?
    } else if kind == database::schema::ALIAS_ENTRY_KIND {
        aliases::table
            .find((site_id, path))
            .select(aliases::site_id)
            .first::<i64>(db)?;
        0
    } else {
        return Err(StoreError::NotFound);
    };
    Ok(size.cast_unsigned())
}

fn finish_entry_mutation(
    tx: &mut SqliteConnection,
    blob_files: &BlobFiles,
    site_id: i64,
    paths: &[&str],
    now: i64,
) -> Result<(), StoreError> {
    diesel::update(sites::table.find(site_id))
        .set((
            sites::updated.eq(now),
            sites::content_revision.eq(sites::content_revision + 1),
        ))
        .execute(tx)?;
    let alias_changes = paths
        .iter()
        .map(|path| AliasChange::Entry(path))
        .collect::<Vec<_>>();
    refresh_aliases_locked(tx, site_id, &alias_changes)?;
    refresh_expiry_for_changes_locked(tx, site_id, paths, now)?;
    regenerate_site(tx, blob_files, site_id, now)?;
    prune_undo_locked(tx, now)?;
    Ok(())
}

fn prune_pending_locked(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<usize, diesel::result::Error> {
    diesel::delete(pending_allocations::table.filter(pending_allocations::expires.le(now)))
        .execute(tx)
}

fn prepare_splices(
    splices: &[Splice<'_>],
    temporary: &Path,
    maximum_staged_bytes: u64,
) -> Result<Vec<PreparedSplice>, StoreError> {
    let mut prepared = Vec::with_capacity(splices.len());
    let mut staged_bytes = 0_u64;
    for (index, splice) in splices.iter().enumerate() {
        let (insert, sanitized) = match splice.insert {
            SpliceSource::Empty => (PreparedSpliceSource::Empty, TokenCounts::default()),
            SpliceSource::Bytes(bytes) => {
                let redacted = sanitize::redact_tokens(bytes);
                let bytes = redacted.as_bytes();
                staged_bytes = staged_bytes
                    .checked_add(u64::try_from(bytes.len()).expect("slice length fits in u64"))
                    .ok_or(StoreError::SpliceResultTooLarge)?;
                if staged_bytes > maximum_staged_bytes {
                    return Err(StoreError::SpliceResultTooLarge);
                }
                (
                    PreparedSpliceSource::Bytes(bytes.to_vec()),
                    redacted.counts(),
                )
            }
            SpliceSource::File(path) => {
                let copied = temporary.join(format!("insert-{index}"));
                let mut source = fs::File::open(path)?;
                let mut target = fs::File::create(&copied)?;
                let remaining = maximum_staged_bytes
                    .checked_sub(staged_bytes)
                    .ok_or(StoreError::SpliceResultTooLarge)?;
                let mut limited = (&mut source).take(remaining.saturating_add(1));
                let size = io::copy(&mut limited, &mut target)?;
                if size > remaining {
                    return Err(StoreError::SpliceResultTooLarge);
                }
                staged_bytes += size;
                target.sync_all()?;
                drop(target);
                let (sanitized_size, hash, sanitized) = sanitized_file_properties(&copied)?;
                let sanitized_size = sanitized_size.cast_unsigned();
                debug_assert_eq!(sanitized_size, size);
                (
                    PreparedSpliceSource::File {
                        path: copied,
                        size: sanitized_size,
                        hash,
                    },
                    sanitized,
                )
            }
        };
        prepared.push(PreparedSplice {
            offset: splice.offset,
            delete: splice.delete,
            insert,
            sanitized,
        });
    }
    Ok(prepared)
}

fn splice_request_fingerprint(
    site: &str,
    path: &str,
    base_hash: &str,
    splices: &[PreparedSplice],
    expected_tree_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-splice-request-v1\0");
    for value in [site, path, base_hash] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    for splice in splices {
        hasher.update(&splice.offset.to_le_bytes());
        hasher.update(&splice.delete.to_le_bytes());
        match &splice.insert {
            PreparedSpliceSource::Empty => {
                hasher.update(&[0]);
            }
            PreparedSpliceSource::Bytes(bytes) => {
                hasher.update(&[1]);
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(blake3::hash(bytes).as_bytes());
            }
            PreparedSpliceSource::File { size, hash, .. } => {
                hasher.update(&[2]);
                hasher.update(&size.to_le_bytes());
                hasher.update(hash.as_bytes());
            }
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn splice_blob_to_path(
    source: &Path,
    old_size: u64,
    splices: &[PreparedSplice],
    output: &Path,
    maximum_result_size: u64,
) -> Result<(String, u64), StoreError> {
    let mut previous_end = 0_u64;
    let mut result_size = old_size;
    for splice in splices {
        let end = splice
            .offset
            .checked_add(splice.delete)
            .ok_or(StoreError::SpliceRange)?;
        if splice.offset < previous_end {
            return Err(StoreError::InvalidSpliceOrder);
        }
        if splice.offset > old_size || end > old_size {
            return Err(StoreError::SpliceRange);
        }
        previous_end = end;
        let insertion_size = match &splice.insert {
            PreparedSpliceSource::Empty => 0,
            PreparedSpliceSource::Bytes(bytes) => {
                u64::try_from(bytes.len()).expect("slice length fits in u64")
            }
            PreparedSpliceSource::File { size, .. } => *size,
        };
        result_size = result_size
            .checked_sub(splice.delete)
            .and_then(|size| size.checked_add(insertion_size))
            .ok_or(StoreError::SpliceResultTooLarge)?;
        if result_size > maximum_result_size {
            return Err(StoreError::SpliceResultTooLarge);
        }
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(output)?;
    let mut hasher = blake3::Hasher::new();
    let mut cursor = 0_u64;
    for splice in splices {
        copy_hashed(&mut input, &mut output, &mut hasher, splice.offset - cursor)?;
        input.seek(SeekFrom::Current(
            i64::try_from(splice.delete).map_err(|_| StoreError::SpliceRange)?,
        ))?;
        match &splice.insert {
            PreparedSpliceSource::Empty => {}
            PreparedSpliceSource::Bytes(bytes) => {
                output.write_all(bytes)?;
                hasher.update(bytes);
            }
            PreparedSpliceSource::File { path, .. } => {
                let mut insertion = fs::File::open(path)?;
                copy_all_hashed(&mut insertion, &mut output, &mut hasher)?;
            }
        }
        cursor = splice.offset + splice.delete;
    }
    copy_hashed(&mut input, &mut output, &mut hasher, old_size - cursor)?;
    output.sync_all()?;
    Ok((hasher.finalize().to_hex().to_string(), result_size))
}

fn copy_hashed(
    input: &mut fs::File,
    output: &mut fs::File,
    hasher: &mut blake3::Hasher,
    mut remaining: u64,
) -> io::Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded read size fits in usize");
        input.read_exact(&mut buffer[..wanted])?;
        output.write_all(&buffer[..wanted])?;
        hasher.update(&buffer[..wanted]);
        remaining -= u64::try_from(wanted).expect("buffer size fits in u64");
    }
    Ok(())
}

fn copy_all_hashed(
    input: &mut impl Read,
    output: &mut fs::File,
    hasher: &mut blake3::Hasher,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            return Ok(size);
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        size += u64::try_from(read).expect("read size fits in u64");
    }
}

fn reject_reserved_path(path: &str) -> Result<(), StoreError> {
    if is_reserved_path(path) || is_virtual_namespace(path) {
        Err(StoreError::Upload(UploadError::ReservedPath))
    } else {
        Ok(())
    }
}

fn is_virtual_namespace(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|component| matches!(component, "FILES" | "UNDO" | "EXPIRES"))
}

fn canonical_alias_target(alias_path: &str, target: &str) -> Result<String, StoreError> {
    if target.is_empty()
        || target.len() > MAX_ALIAS_TARGET_BYTES
        || target.starts_with(['/', '\\'])
        || target.contains('\\')
        || target.chars().any(char::is_control)
        || looks_like_external_alias_target(target)
    {
        return Err(StoreError::InvalidAliasTarget);
    }
    let mut parts = alias_path
        .rsplit_once('/')
        .map_or_else(Vec::new, |(parent, _)| {
            parent.split('/').collect::<Vec<_>>()
        });
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(StoreError::InvalidAliasTarget);
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(StoreError::InvalidAliasTarget);
    }
    let canonical = parts.join("/");
    safe_rel_path(&canonical).map_err(|_| StoreError::InvalidAliasTarget)?;
    if is_reserved_path(&canonical) || is_noise_path(Path::new(&canonical)) {
        return Err(StoreError::InvalidAliasTarget);
    }
    if relative_alias_target(alias_path, &canonical).len() > MAX_ALIAS_TARGET_BYTES {
        return Err(StoreError::InvalidAliasTarget);
    }
    Ok(canonical)
}

fn looks_like_external_alias_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.contains("://")
        || [
            "data:",
            "file:",
            "ftp:",
            "ftps:",
            "git:",
            "http:",
            "https:",
            "javascript:",
            "mailto:",
            "ssh:",
            "ws:",
            "wss:",
        ]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

fn alias_mutation_fingerprint(
    name: &str,
    aliases: &BTreeMap<String, String>,
    expected_tree_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-alias-mutation-v1\0");
    hasher.update(name.as_bytes());
    for (path, target) in aliases {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(target.len() as u64).to_le_bytes());
        hasher.update(target.as_bytes());
    }
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn load_requested_aliases_locked<'a>(
    tx: &mut SqliteConnection,
    site_id: i64,
    paths: impl Iterator<Item = &'a String>,
) -> Result<Vec<AliasEntry>, StoreError> {
    let paths = paths.cloned().collect::<BTreeSet<_>>();
    aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(tx)?
        .into_iter()
        .filter(|row| paths.contains(&row.path))
        .map(alias_entry)
        .collect()
}

fn alias_mutation_replay(
    tx: &mut SqliteConnection,
    idempotency: &Idempotency,
    fingerprint: &str,
) -> Result<Option<AliasMutationResult>, StoreError> {
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != IdempotencyKind::AliasMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<AliasMutationResult>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    result.mutation.replayed = true;
    Ok(Some(result))
}

fn store_alias_mutation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AliasMutationResult,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::AliasMutation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn alias_entry(row: AliasRow) -> Result<AliasEntry, StoreError> {
    let resolved_kind = row
        .resolved_kind
        .map(|kind| match kind {
            0 => Ok(AliasResolvedKind::File),
            1 => Ok(AliasResolvedKind::Directory),
            _ => Err(StoreError::InvalidAliasTarget),
        })
        .transpose()?;
    Ok(AliasEntry {
        path: row.path,
        canonical_target: row.canonical_target,
        resolved_kind,
        resolved_hash: row.resolved_hash,
        resolved_size: row.resolved_size.map(i64::cast_unsigned),
        resolved_files: None,
    })
}

fn is_reserved_path(path: &str) -> bool {
    let terminal = path.rsplit('/').next().unwrap_or(path);
    RESERVED_TERMINALS.contains(&terminal)
}

fn validate_idempotency_key(key: &str) -> Result<(), StoreError> {
    if key.is_empty()
        || key.len() > 256
        || !key
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(StoreError::InvalidIdempotencyKey);
    }
    Ok(())
}

fn idempotency_key_hash(key: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-idempotency-v1\0");
    hasher.update(key.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn pending_allocation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    site_id: i64,
    now: i64,
) -> Result<Option<PendingAllocation>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != IdempotencyKind::PendingAllocation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<PendingAllocation>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    let exists = pending_allocations::table
        .find(&result.token)
        .filter(pending_allocations::site_id.eq(site_id))
        .filter(pending_allocations::expires.gt(now))
        .select(pending_allocations::token)
        .first::<String>(tx)
        .optional()?
        .is_some();
    if !exists {
        return Err(StoreError::InvalidPendingAllocation);
    }
    result.replayed = true;
    Ok(Some(result))
}

fn cancellation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<Option<AllocationCancellation>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint
        || stored_kind != IdempotencyKind::AllocationCancellation as i64
    {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<AllocationCancellation>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    result.replayed = true;
    Ok(Some(result))
}

fn store_cancellation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AllocationCancellation,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    validate_idempotency_key(&idempotency.key)?;
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::AllocationCancellation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn store_pending_allocation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &PendingAllocation,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    validate_idempotency_key(&idempotency.key)?;
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::PendingAllocation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn staged_entries_fingerprint(
    files: &[&StagedFile],
    archive_aliases: &[ArchiveAlias<'_>],
) -> String {
    let mut files = files.to_vec();
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-unnamed-put-v1\0");
    for file in files {
        hasher.update(&(file.path.len() as u64).to_le_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update(file.hash.as_bytes());
    }
    for alias in archive_aliases {
        hasher.update(b"\0alias\0");
        hasher.update(&(alias.path.len() as u64).to_le_bytes());
        hasher.update(alias.path.as_bytes());
        hasher.update(&(alias.target.len() as u64).to_le_bytes());
        hasher.update(alias.target.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn sanitized_counts(files: &[&StagedFile]) -> TokenCounts {
    files
        .iter()
        .fold(TokenCounts::default(), |mut total, file| {
            total.management += file.sanitized.management;
            total.claim += file.sanitized.claim;
            total
        })
}

fn idempotency_replay(
    tx: &mut SqliteConnection,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
) -> Result<Option<PublishedMutation>, StoreError> {
    let key_hash = idempotency_key_hash(key);
    let record = idempotency_records::table
        .find(key_hash)
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || stored_kind != kind as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut replay: PublishedMutation = serde_json::from_str(&metadata)
        .map_err(|err| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    replay.mutation.replayed = true;
    Ok(Some(replay))
}

fn store_idempotency(
    tx: &mut SqliteConnection,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
    result: &PublishedMutation,
    now: i64,
) -> Result<(), StoreError> {
    let metadata = serde_json::to_string(result)
        .map_err(|err| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(kind as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn entry_mutation_fingerprint(
    name: &str,
    current_path: Option<&str>,
    destination: &str,
    hash: &str,
    kind: UndoKind,
    expected_tree_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-entry-mutation-v1\0");
    hasher.update(name.as_bytes());
    hasher.update(&[0]);
    hasher.update(current_path.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    hasher.update(destination.as_bytes());
    hasher.update(&[0]);
    hasher.update(hash.as_bytes());
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    hasher.update(&(kind as i64).to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn content_mutation_request_fingerprint(
    name: &str,
    current_path: &str,
    hash: &str,
    expected_content_hash: Option<&str>,
    expected_tree_hash: Option<&str>,
    kind: UndoKind,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-content-mutation-request-v1\0");
    for value in [
        name,
        current_path,
        hash,
        expected_content_hash.unwrap_or(""),
        expected_tree_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&(kind as i64).to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn allocated_entry_mutation_fingerprint(input: &AllocatedEntryFingerprint<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-allocated-entry-mutation-v2\0");
    for value in [
        input.name,
        input.current_path.unwrap_or(""),
        &input.destination.path,
        input.hash,
        &input.destination.prefix,
        &input.destination.suffix,
        input.destination.extension.as_deref().unwrap_or(""),
        &input.destination.media_type,
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&(input.destination.naming_mode as i64).to_le_bytes());
    hasher.update(&(input.kind as i64).to_le_bytes());
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn entry_mutation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<Option<AllocatedFile>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || stored_kind != IdempotencyKind::EntryMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut record: EntryMutationRecord = serde_json::from_str(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    record.result.replayed = true;
    if let Some(mutation) = &mut record.result.mutation {
        mutation.replayed = true;
    }
    Ok(Some(record.result))
}

fn entry_mutation_request_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    request_fingerprint: &str,
) -> Result<Option<AllocatedFile>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(i64, String)>(tx)
        .optional()?;
    let Some((stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_kind != IdempotencyKind::EntryMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut record: EntryMutationRecord = serde_json::from_str(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    if record.request_fingerprint != request_fingerprint {
        return Err(StoreError::IdempotencyConflict);
    }
    record.result.replayed = true;
    if let Some(mutation) = &mut record.result.mutation {
        mutation.replayed = true;
    }
    Ok(Some(record.result))
}

fn store_entry_mutation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AllocatedFile,
    now: i64,
) -> Result<(), StoreError> {
    store_entry_mutation_with_request(tx, idempotency, fingerprint, fingerprint, result, now)
}

fn store_entry_mutation_with_request(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    request_fingerprint: &str,
    result: &AllocatedFile,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    let metadata = serde_json::to_string(&EntryMutationRecord {
        result: result.clone(),
        request_fingerprint: request_fingerprint.to_string(),
    })
    .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::EntryMutation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn prune_idempotency_locked(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<(), diesel::result::Error> {
    diesel::delete(idempotency_records::table.filter(idempotency_records::expires.le(now)))
        .execute(tx)?;
    Ok(())
}

fn management_hash_from_blob(bytes: &[u8]) -> Result<ManagementTokenHash, StoreError> {
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid stored management hash")
    })?;
    Ok(ManagementTokenHash::from_bytes(bytes))
}

fn authorize_locked(
    db: &mut SqliteConnection,
    name: &str,
    token: Option<&ManagementToken>,
) -> Result<(), StoreError> {
    let live = sites::table
        .filter(sites::name.eq(name))
        .select((sites::management_status, sites::management_hash))
        .first::<(i64, Option<Vec<u8>>)>(db)
        .optional()?;
    let expected = match live {
        Some((0, _)) => return Ok(()),
        Some((_, hash)) => hash,
        None => management_tombstones::table
            .find(name)
            .select(management_tombstones::management_hash)
            .first::<Vec<u8>>(db)
            .optional()?,
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    let candidate = token.ok_or(StoreError::Unauthorized)?;
    if management_hash_from_blob(&expected)?.verify(candidate) {
        Ok(())
    } else {
        Err(StoreError::Unauthorized)
    }
}

fn claim_hash_from_blob(bytes: &[u8]) -> Result<ClaimTokenHash, StoreError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid stored claim hash"))?;
    Ok(ClaimTokenHash::from_bytes(bytes))
}

fn management_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<bool, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(false);
    };
    validate_idempotency_key(&idempotency.key)?;
    let stored = management_idempotency::table
        .find(idempotency_key_hash(&idempotency.key))
        .select(management_idempotency::fingerprint)
        .first::<String>(tx)
        .optional()?;
    match stored {
        Some(stored) if stored == fingerprint => Ok(true),
        Some(_) => Err(StoreError::IdempotencyConflict),
        None => Ok(false),
    }
}

fn store_management_idempotency(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    diesel::insert_into(management_idempotency::table)
        .values((
            management_idempotency::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            management_idempotency::fingerprint.eq(fingerprint),
            management_idempotency::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

fn prune_management_idempotency(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<(), diesel::result::Error> {
    diesel::delete(management_idempotency::table.filter(management_idempotency::expires.le(now)))
        .execute(tx)?;
    Ok(())
}

fn record_management(
    tx: &mut SqliteConnection,
    name: &str,
    action: i64,
    now: i64,
    source_ip: Option<&str>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(management_audit::table)
        .values((
            management_audit::site_name.eq(name),
            management_audit::action.eq(action),
            management_audit::occurred.eq(now),
            management_audit::source_ip.eq(source_ip),
        ))
        .execute(tx)?;
    Ok(())
}

fn snapshot_site(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let description = match kind {
        UndoKind::Put | UndoKind::PutFile => format!("restore previous state of {name}"),
        UndoKind::DeletePath | UndoKind::DeleteSite => format!("restore deleted site {name}"),
        UndoKind::Copy => format!("remove copied site {name}"),
        UndoKind::Move => format!("restore previous name {name}"),
        UndoKind::Expiry => format!("restore previous expiry policy for {name}"),
        UndoKind::ExpireSweep => format!("restore expired content in {name}"),
        UndoKind::Allocate => format!("remove allocated content from {name}"),
        UndoKind::Replace => format!("restore replaced content in {name}"),
        UndoKind::Splice => format!("restore spliced content in {name}"),
        UndoKind::Alias => format!("restore aliases in {name}"),
    };
    snapshot_site_with_description(tx, name, kind, &description, now)
}

fn insert_undo_allocated(
    tx: &mut SqliteConnection,
    token: &str,
    path: &str,
    metadata: Option<&AllocatedMetadata>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(undo_allocated_deltas::table)
        .values((
            undo_allocated_deltas::token.eq(token),
            undo_allocated_deltas::path.eq(path),
            undo_allocated_deltas::existed.eq(i64::from(metadata.is_some())),
            undo_allocated_deltas::hash.eq(metadata.map(|value| value.hash.as_str())),
            undo_allocated_deltas::size.eq(metadata.map(|value| value.size)),
            undo_allocated_deltas::naming_mode.eq(metadata.map(|value| value.naming_mode as i64)),
            undo_allocated_deltas::prefix.eq(metadata.map(|value| value.prefix.as_str())),
            undo_allocated_deltas::suffix.eq(metadata.map(|value| value.suffix.as_str())),
            undo_allocated_deltas::extension
                .eq(metadata.and_then(|value| value.extension.as_deref())),
            undo_allocated_deltas::media_type.eq(metadata.map(|value| value.media_type.as_str())),
        ))
        .execute(tx)?;
    Ok(())
}

fn insert_undo_alias(
    tx: &mut SqliteConnection,
    token: &str,
    path: &str,
    alias: Option<&AliasRow>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(undo_alias_deltas::table)
        .values((
            undo_alias_deltas::token.eq(token),
            undo_alias_deltas::path.eq(path),
            undo_alias_deltas::existed.eq(i64::from(alias.is_some())),
            undo_alias_deltas::canonical_target.eq(alias.map(|row| row.canonical_target.as_str())),
            undo_alias_deltas::resolved_kind.eq(alias.and_then(|row| row.resolved_kind)),
            undo_alias_deltas::resolved_hash.eq(alias.and_then(|row| row.resolved_hash.as_deref())),
            undo_alias_deltas::resolved_size.eq(alias.and_then(|row| row.resolved_size)),
        ))
        .execute(tx)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn snapshot_site_with_description(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    description: &str,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let token = undo_token()?;
    let expires = now + UNDO_RETENTION_MILLIS;
    diesel::insert_into(undo_operations::table)
        .values((
            undo_operations::token.eq(&token),
            undo_operations::kind.eq(kind as i64),
            undo_operations::description.eq(description),
            undo_operations::created.eq(now),
            undo_operations::expires.eq(expires),
            undo_operations::consumed.eq(0_i64),
        ))
        .execute(tx)?;
    diesel::insert_into(undo_names::table)
        .values((undo_names::token.eq(&token), undo_names::name.eq(name)))
        .execute(tx)?;
    let site = sites::table
        .filter(sites::name.eq(name))
        .select((
            sites::name,
            sites::public_url,
            sites::updated,
            sites::content_revision,
            sites::tree_hash,
        ))
        .first::<(String, String, i64, i64, String)>(tx)
        .optional()?
        .map_or_else(
            || SiteSnapshot {
                name: name.to_string(),
                existed: false,
                public_url: String::new(),
                updated: now,
                content_revision: 0,
                tree_hash: String::new(),
            },
            |(name, public_url, updated, content_revision, tree_hash)| SiteSnapshot {
                name,
                existed: true,
                public_url,
                updated,
                content_revision,
                tree_hash,
            },
        );
    diesel::insert_into(undo_sites::table)
        .values((
            undo_sites::token.eq(&token),
            undo_sites::name.eq(&site.name),
            undo_sites::existed.eq(i64::from(site.existed)),
            undo_sites::public_url.eq(&site.public_url),
            undo_sites::updated.eq(site.updated),
            undo_sites::content_revision.eq(site.content_revision),
            undo_sites::tree_hash.eq(&site.tree_hash),
        ))
        .execute(tx)?;
    if site.existed {
        let site_id = site_id_locked(tx, name)?;
        let saved_files = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .load::<(String, String, i64)>(tx)?;
        for (path, hash, size) in saved_files {
            diesel::insert_into(undo_files::table)
                .values((
                    undo_files::token.eq(&token),
                    undo_files::path.eq(path),
                    undo_files::hash.eq(hash),
                    undo_files::size.eq(size),
                ))
                .execute(tx)?;
        }
        let saved_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
                allocated_entries::naming_mode,
                allocated_entries::prefix,
                allocated_entries::suffix,
                allocated_entries::extension,
                allocated_entries::media_type,
            ))
            .load::<(
                String,
                String,
                i64,
                i64,
                String,
                String,
                Option<String>,
                String,
            )>(tx)?;
        for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
            saved_allocated
        {
            insert_undo_allocated(
                tx,
                &token,
                &path,
                Some(&AllocatedMetadata {
                    hash,
                    size,
                    naming_mode: AllocatedNamingMode::try_from(naming_mode)?,
                    prefix,
                    suffix,
                    extension,
                    media_type,
                }),
            )?;
        }
        let saved_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .load::<AliasRow>(tx)?;
        for alias in saved_aliases {
            insert_undo_alias(tx, &token, &alias.path, Some(&alias))?;
        }
        snapshot_expiry_policies_locked(tx, &token, site_id)?;
    }
    Ok(UndoInfo {
        token,
        expires_at: format_timestamp(expires),
    })
}

#[allow(clippy::too_many_lines)]
fn snapshot_entry_deltas(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    description: &str,
    paths: &[&str],
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let token = undo_token()?;
    let expires = now + UNDO_RETENTION_MILLIS;
    diesel::insert_into(undo_operations::table)
        .values((
            undo_operations::token.eq(&token),
            undo_operations::kind.eq(kind as i64),
            undo_operations::description.eq(description),
            undo_operations::created.eq(now),
            undo_operations::expires.eq(expires),
            undo_operations::consumed.eq(0_i64),
        ))
        .execute(tx)?;
    diesel::insert_into(undo_names::table)
        .values((undo_names::token.eq(&token), undo_names::name.eq(name)))
        .execute(tx)?;
    let (site_id, public_url, updated, content_revision, tree_hash) = sites::table
        .filter(sites::name.eq(name))
        .select((
            sites::id,
            sites::public_url,
            sites::updated,
            sites::content_revision,
            sites::tree_hash,
        ))
        .first::<(i64, String, i64, i64, String)>(tx)?;
    diesel::insert_into(undo_sites::table)
        .values((
            undo_sites::token.eq(&token),
            undo_sites::name.eq(name),
            undo_sites::existed.eq(1_i64),
            undo_sites::public_url.eq(public_url),
            undo_sites::updated.eq(updated),
            undo_sites::content_revision.eq(content_revision),
            undo_sites::tree_hash.eq(tree_hash),
        ))
        .execute(tx)?;
    if matches!(kind, UndoKind::Alias) {
        let mut previous = HashMap::new();
        for chunk in paths.chunks(SQLITE_DELETE_BATCH_SIZE) {
            previous.extend(
                aliases::table
                    .filter(aliases::site_id.eq(site_id))
                    .filter(aliases::path.eq_any(chunk))
                    .select(AliasRow::as_select())
                    .load::<AliasRow>(tx)?
                    .into_iter()
                    .map(|row| (row.path.clone(), row)),
            );
        }
        for path in paths {
            insert_undo_alias(tx, &token, path, previous.get(*path))?;
        }
        snapshot_expiry_policies_locked(tx, &token, site_id)?;
        return Ok(UndoInfo {
            token,
            expires_at: format_timestamp(expires),
        });
    }
    let operation_is_allocated = matches!(kind, UndoKind::Allocate)
        || (matches!(kind, UndoKind::Replace | UndoKind::Splice)
            && site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(paths))
                .filter(site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND))
                .select(count_star())
                .first::<i64>(tx)?
                > 0);
    for path in paths {
        let entry_kind = site_entries::table
            .find((site_id, *path))
            .select(site_entries::kind)
            .first::<i64>(tx)
            .optional()?;
        match entry_kind {
            Some(entry_kind) if entry_kind == database::schema::FILE_ENTRY_KIND => {
                let (hash, size) = files::table
                    .find((site_id, *path))
                    .select((files::hash, files::size))
                    .first::<(String, i64)>(tx)?;
                diesel::insert_into(undo_file_deltas::table)
                    .values((
                        undo_file_deltas::token.eq(&token),
                        undo_file_deltas::path.eq(*path),
                        undo_file_deltas::existed.eq(1_i64),
                        undo_file_deltas::kind.eq(Some(entry_kind)),
                        undo_file_deltas::hash.eq(Some(hash)),
                        undo_file_deltas::size.eq(Some(size)),
                    ))
                    .execute(tx)?;
            }
            Some(entry_kind) if entry_kind == database::schema::ALLOCATED_ENTRY_KIND => {
                let metadata = allocated_metadata_locked(tx, site_id, path)?;
                insert_undo_allocated(tx, &token, path, Some(&metadata))?;
            }
            Some(entry_kind) if entry_kind == database::schema::ALIAS_ENTRY_KIND => {
                let alias = aliases::table
                    .find((site_id, *path))
                    .select(AliasRow::as_select())
                    .first::<AliasRow>(tx)?;
                insert_undo_alias(tx, &token, path, Some(&alias))?;
            }
            Some(_) => return Err(StoreError::DestinationConflict),
            None if matches!(kind, UndoKind::Alias) => {
                insert_undo_alias(tx, &token, path, None)?;
            }
            None if operation_is_allocated => {
                insert_undo_allocated(tx, &token, path, None)?;
            }
            None => {
                diesel::insert_into(undo_file_deltas::table)
                    .values((
                        undo_file_deltas::token.eq(&token),
                        undo_file_deltas::path.eq(*path),
                        undo_file_deltas::existed.eq(0_i64),
                        undo_file_deltas::kind.eq(Option::<i64>::None),
                        undo_file_deltas::hash.eq(Option::<String>::None),
                        undo_file_deltas::size.eq(Option::<i64>::None),
                    ))
                    .execute(tx)?;
            }
        }
    }
    snapshot_expiry_policies_locked(tx, &token, site_id)?;
    Ok(UndoInfo {
        token,
        expires_at: format_timestamp(expires),
    })
}

#[allow(clippy::too_many_lines)]
fn restore_entry_deltas(
    tx: &mut SqliteConnection,
    blob_files: &BlobFiles,
    name: &str,
    token: &str,
) -> Result<i64, StoreError> {
    let (updated, content_revision) = undo_sites::table
        .find(token)
        .select((undo_sites::updated, undo_sites::content_revision))
        .first::<(i64, i64)>(tx)?;
    let site_id = site_id_locked(tx, name)?;
    let file_deltas = undo_file_deltas::table
        .filter(undo_file_deltas::token.eq(token))
        .select((
            undo_file_deltas::path,
            undo_file_deltas::existed,
            undo_file_deltas::hash,
            undo_file_deltas::size,
        ))
        .load::<(String, i64, Option<String>, Option<i64>)>(tx)?;
    let allocated_deltas = undo_allocated_deltas::table
        .filter(undo_allocated_deltas::token.eq(token))
        .select(UndoAllocatedMetadata::as_select())
        .load::<UndoAllocatedMetadata>(tx)?;
    let alias_deltas = undo_alias_deltas::table
        .filter(undo_alias_deltas::token.eq(token))
        .select(UndoAliasRow::as_select())
        .load::<UndoAliasRow>(tx)?;
    let entry_change_paths = file_deltas
        .iter()
        .map(|row| row.0.clone())
        .chain(allocated_deltas.iter().map(|row| row.path.clone()))
        .collect::<Vec<_>>();
    let alias_change_paths = alias_deltas
        .iter()
        .map(|row| row.path.clone())
        .collect::<Vec<_>>();
    let changed_paths = entry_change_paths
        .iter()
        .chain(&alias_change_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    for paths in changed_paths.chunks(512) {
        diesel::delete(
            site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(paths)),
        )
        .execute(tx)?;
    }
    for (path, existed, hash, size) in file_deltas {
        if existed == 0 {
            continue;
        }
        let hash = hash.expect("existing file delta has hash");
        let size = size.expect("existing file delta has size");
        ensure_file_entry(tx, site_id, &path)?;
        diesel::insert_into(files::table)
            .values(NewFile {
                site_id,
                path: &path,
                hash: &hash,
                size,
            })
            .execute(tx)?;
    }
    for delta in allocated_deltas {
        if delta.existed == 0 {
            continue;
        }
        let path = delta.path;
        diesel::insert_into(site_entries::table)
            .values((
                site_entries::site_id.eq(site_id),
                site_entries::path.eq(&path),
                site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
            ))
            .execute(tx)?;
        diesel::insert_into(allocated_entries::table)
            .values((
                allocated_entries::site_id.eq(site_id),
                allocated_entries::path.eq(&path),
                allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                allocated_entries::hash.eq(delta.hash.expect("existing allocated delta has hash")),
                allocated_entries::size.eq(delta.size.expect("existing allocated delta has size")),
                allocated_entries::naming_mode.eq(delta
                    .naming_mode
                    .expect("existing allocated delta has naming mode")),
                allocated_entries::prefix
                    .eq(delta.prefix.expect("existing allocated delta has prefix")),
                allocated_entries::suffix
                    .eq(delta.suffix.expect("existing allocated delta has suffix")),
                allocated_entries::extension.eq(delta.extension),
                allocated_entries::media_type.eq(delta
                    .media_type
                    .expect("existing allocated delta has media type")),
            ))
            .execute(tx)?;
    }
    for delta in alias_deltas {
        if delta.existed == 0 {
            continue;
        }
        let path = delta.path;
        diesel::insert_into(site_entries::table)
            .values((
                site_entries::site_id.eq(site_id),
                site_entries::path.eq(&path),
                site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
            ))
            .execute(tx)?;
        diesel::insert_into(aliases::table)
            .values((
                aliases::site_id.eq(site_id),
                aliases::path.eq(&path),
                aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                aliases::canonical_target.eq(delta
                    .canonical_target
                    .expect("existing alias delta has target")),
                aliases::resolved_kind.eq(delta.resolved_kind),
                aliases::resolved_hash.eq(delta.resolved_hash),
                aliases::resolved_size.eq(delta.resolved_size),
            ))
            .execute(tx)?;
    }
    diesel::delete(expiry_policies::table.filter(expiry_policies::site_id.eq(site_id)))
        .execute(tx)?;
    restore_expiry_policies_locked(tx, token, site_id)?;
    rebuild_aggregates_locked(tx, site_id)?;
    diesel::update(sites::table.find(site_id))
        .set(sites::content_revision.eq(content_revision))
        .execute(tx)?;
    let alias_changes = entry_change_paths
        .iter()
        .map(|path| AliasChange::Entry(path))
        .chain(
            alias_change_paths
                .iter()
                .map(|path| AliasChange::Alias(path)),
        )
        .collect::<Vec<_>>();
    refresh_aliases_locked(tx, site_id, &alias_changes)?;
    regenerate_site(tx, blob_files, site_id, updated)?;
    Ok(updated)
}

fn snapshot_expiry_policies_locked(
    db: &mut SqliteConnection,
    token: &str,
    site_id: i64,
) -> Result<(), StoreError> {
    let policies = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select(ExpiryPolicyRow::as_select())
        .load::<ExpiryPolicyRow>(db)?;
    for policy in policies {
        diesel::insert_into(undo_expiry_policies::table)
            .values((
                undo_expiry_policies::token.eq(token),
                undo_expiry_policies::path.eq(policy.path),
                undo_expiry_policies::target_kind.eq(policy.target_kind),
                undo_expiry_policies::mode.eq(policy.mode),
                undo_expiry_policies::duration_seconds.eq(policy.duration_seconds),
                undo_expiry_policies::deadline.eq(policy.deadline),
                undo_expiry_policies::min_age_seconds.eq(policy.min_age_seconds),
                undo_expiry_policies::max_age_seconds.eq(policy.max_age_seconds),
                undo_expiry_policies::max_size_bytes.eq(policy.max_size_bytes),
                undo_expiry_policies::power.eq(policy.power),
                undo_expiry_policies::refreshed.eq(policy.refreshed),
                undo_expiry_policies::own_deadline.eq(policy
                    .own_deadline
                    .expect("stored expiry deadline is present")),
                undo_expiry_policies::size_bytes.eq(policy.size_bytes),
            ))
            .execute(db)?;
    }
    Ok(())
}

fn restore_expiry_policies_locked(
    db: &mut SqliteConnection,
    token: &str,
    site_id: i64,
) -> Result<(), StoreError> {
    let policies = undo_expiry_policies::table
        .filter(undo_expiry_policies::token.eq(token))
        .select(UndoExpiryPolicyRow::as_select())
        .load::<UndoExpiryPolicyRow>(db)?;
    for policy in policies {
        diesel::insert_into(expiry_policies::table)
            .values((
                expiry_policies::site_id.eq(site_id),
                expiry_policies::path.eq(policy.path),
                expiry_policies::target_kind.eq(policy.target_kind),
                expiry_policies::mode.eq(policy.mode),
                expiry_policies::duration_seconds.eq(policy.duration_seconds),
                expiry_policies::deadline.eq(policy.deadline),
                expiry_policies::min_age_seconds.eq(policy.min_age_seconds),
                expiry_policies::max_age_seconds.eq(policy.max_age_seconds),
                expiry_policies::max_size_bytes.eq(policy.max_size_bytes),
                expiry_policies::power.eq(policy.power),
                expiry_policies::refreshed.eq(policy.refreshed),
                expiry_policies::own_deadline.eq(Some(policy.own_deadline)),
                expiry_policies::size_bytes.eq(policy.size_bytes),
            ))
            .execute(db)?;
    }
    Ok(())
}

fn expiry_display_path(name: &str, path: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{name}/{path}")
    }
}

fn expiry_target_kind_locked(
    db: &mut SqliteConnection,
    name: &str,
    rel: &str,
) -> Result<ExpiryTargetKind, StoreError> {
    if !rel.is_empty() {
        let site_id = site_id_locked(db, name)?;
        let alias_kind = aliases::table
            .find((site_id, rel))
            .select(aliases::resolved_kind)
            .first::<Option<i64>>(db)
            .optional()?;
        if let Some(kind) = alias_kind {
            return match kind {
                Some(kind) if kind == AliasResolvedKind::Directory as i64 => {
                    Ok(ExpiryTargetKind::Folder)
                }
                Some(kind) if kind == AliasResolvedKind::File as i64 => Ok(ExpiryTargetKind::File),
                None => expiry_policies::table
                    .find((site_id, rel))
                    .select(expiry_policies::target_kind)
                    .first::<i64>(db)
                    .optional()?
                    .map_or(Ok(ExpiryTargetKind::File), ExpiryTargetKind::try_from)
                    .map_err(StoreError::Expiry),
                Some(_) => Err(StoreError::InvalidAliasTarget),
            };
        }
    }
    match node_locked(db, name, rel)? {
        NodeKind::Missing => Err(StoreError::NotFound),
        NodeKind::Dir if rel.is_empty() => Ok(ExpiryTargetKind::Site),
        NodeKind::Dir => Ok(ExpiryTargetKind::Folder),
        NodeKind::File { .. } => Ok(ExpiryTargetKind::File),
    }
}

fn aggregate_paths(path: &str) -> Vec<&str> {
    let mut paths = vec![""];
    let mut offset = 0;
    while let Some(relative) = path[offset..].find('/') {
        let end = offset + relative;
        paths.push(&path[..end]);
        offset = end + 1;
    }
    paths
}

fn adjust_aggregates_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    byte_delta: i64,
    count_delta: i64,
) -> Result<(), StoreError> {
    if path == MANIFEST_PATH {
        return Ok(());
    }
    for aggregate_path in aggregate_paths(path) {
        diesel::insert_into(path_aggregates::table)
            .values((
                path_aggregates::site_id.eq(site_id),
                path_aggregates::path.eq(aggregate_path),
                path_aggregates::logical_bytes.eq(byte_delta),
                path_aggregates::file_count.eq(count_delta),
            ))
            .on_conflict((path_aggregates::site_id, path_aggregates::path))
            .do_update()
            .set((
                path_aggregates::logical_bytes
                    .eq(path_aggregates::logical_bytes + excluded(path_aggregates::logical_bytes)),
                path_aggregates::file_count
                    .eq(path_aggregates::file_count + excluded(path_aggregates::file_count)),
            ))
            .execute(tx)?;
    }
    diesel::delete(
        path_aggregates::table
            .filter(path_aggregates::site_id.eq(site_id))
            .filter(
                path_aggregates::logical_bytes
                    .le(0_i64)
                    .or(path_aggregates::file_count.le(0_i64)),
            ),
    )
    .execute(tx)?;
    Ok(())
}

fn rebuild_aggregates_locked(tx: &mut SqliteConnection, site_id: i64) -> Result<(), StoreError> {
    diesel::delete(path_aggregates::table.filter(path_aggregates::site_id.eq(site_id)))
        .execute(tx)?;
    let files = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::size))
        .load::<(String, i64)>(tx)?;
    for (path, size) in files {
        adjust_aggregates_locked(tx, site_id, &path, size, 1)?;
    }
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((allocated_entries::path, allocated_entries::size))
        .load::<(String, i64)>(tx)?;
    for (path, size) in allocated {
        adjust_aggregates_locked(tx, site_id, &path, size, 1)?;
    }
    Ok(())
}

fn expiry_target_size_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    rel: &str,
    kind: ExpiryTargetKind,
) -> Result<u64, StoreError> {
    if !rel.is_empty()
        && aliases::table
            .find((site_id, rel))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some()
    {
        return Ok(0);
    }
    let size = match kind {
        ExpiryTargetKind::Site => path_aggregates::table
            .find((site_id, ""))
            .select(path_aggregates::logical_bytes)
            .first::<i64>(db)
            .optional()?
            .unwrap_or(0),
        ExpiryTargetKind::File => {
            if let Some(size) = files::table
                .find((site_id, rel))
                .select(files::size)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else if let Some(size) = allocated_entries::table
                .find((site_id, rel))
                .select(allocated_entries::size)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else {
                let (resolved, _, used_alias) = resolve_db_path_final(db, site_id, rel)?;
                match resolved {
                    GraphResolution::File(file) if used_alias => file.size,
                    GraphResolution::Missing
                    | GraphResolution::Directory
                    | GraphResolution::File(_) => return Err(StoreError::NotFound),
                }
            }
        }
        ExpiryTargetKind::Folder => {
            if let Some(size) = path_aggregates::table
                .find((site_id, rel))
                .select(path_aggregates::logical_bytes)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else {
                let graph = aliases::table
                    .filter(aliases::site_id.eq(site_id))
                    .select((aliases::path, aliases::canonical_target))
                    .load::<(String, String)>(db)?
                    .into_iter()
                    .collect::<BTreeMap<_, _>>();
                let real_entries = load_real_entries(db, site_id)?;
                let (resolved, target) = resolve_graph_path_final(&real_entries, &graph, rel)?;
                if !matches!(resolved, GraphResolution::Directory) {
                    return Err(StoreError::NotFound);
                }
                path_aggregates::table
                    .find((site_id, target))
                    .select(path_aggregates::logical_bytes)
                    .first::<i64>(db)?
            }
        }
    };
    Ok(size.cast_unsigned())
}

#[allow(clippy::too_many_lines)]
fn store_expiry_policy_locked(
    tx: &mut SqliteConnection,
    write: ExpiryPolicyWrite<'_>,
) -> Result<(), StoreError> {
    let ExpiryPolicyWrite {
        site_id,
        path,
        kind,
        policy,
        size,
        now,
    } = write;
    let (duration, deadline, min_age, max_age, max_size, power, refreshed, own_deadline) =
        match policy {
            ExpiryPolicy::Relative { duration_seconds } => {
                let duration_millis = i64::try_from(duration_seconds)
                    .map_err(|_| ExpiryError::DeadlineOverflow)?
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?;
                (
                    Some(
                        i64::try_from(duration_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    now.checked_add(duration_millis)
                        .ok_or(ExpiryError::DeadlineOverflow)?,
                )
            }
            ExpiryPolicy::Absolute {
                deadline_unix_seconds,
            } => (
                None,
                Some(deadline_unix_seconds),
                None,
                None,
                None,
                None,
                None,
                deadline_unix_seconds
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?,
            ),
            ExpiryPolicy::Decay(decay) => {
                let retention = decay.retention_seconds(size)?;
                let retention_millis = i64::try_from(retention)
                    .map_err(|_| ExpiryError::DeadlineOverflow)?
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?;
                (
                    None,
                    None,
                    Some(
                        i64::try_from(decay.min_age_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(
                        i64::try_from(decay.max_age_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(
                        i64::try_from(decay.max_size_bytes)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(decay.power),
                    Some(now),
                    now.checked_add(retention_millis)
                        .ok_or(ExpiryError::DeadlineOverflow)?,
                )
            }
        };
    diesel::insert_into(expiry_policies::table)
        .values((
            expiry_policies::site_id.eq(site_id),
            expiry_policies::path.eq(path),
            expiry_policies::target_kind.eq(i64::from(kind)),
            expiry_policies::mode.eq(i64::from(policy.mode())),
            expiry_policies::duration_seconds.eq(duration),
            expiry_policies::deadline.eq(deadline),
            expiry_policies::min_age_seconds.eq(min_age),
            expiry_policies::max_age_seconds.eq(max_age),
            expiry_policies::max_size_bytes.eq(max_size),
            expiry_policies::power.eq(power),
            expiry_policies::refreshed.eq(refreshed),
            expiry_policies::own_deadline.eq(Some(own_deadline)),
            expiry_policies::size_bytes
                .eq(i64::try_from(size).map_err(|_| ExpiryError::DeadlineOverflow)?),
        ))
        .on_conflict((expiry_policies::site_id, expiry_policies::path))
        .do_update()
        .set((
            expiry_policies::target_kind.eq(excluded(expiry_policies::target_kind)),
            expiry_policies::mode.eq(excluded(expiry_policies::mode)),
            expiry_policies::duration_seconds.eq(excluded(expiry_policies::duration_seconds)),
            expiry_policies::deadline.eq(excluded(expiry_policies::deadline)),
            expiry_policies::min_age_seconds.eq(excluded(expiry_policies::min_age_seconds)),
            expiry_policies::max_age_seconds.eq(excluded(expiry_policies::max_age_seconds)),
            expiry_policies::max_size_bytes.eq(excluded(expiry_policies::max_size_bytes)),
            expiry_policies::power.eq(excluded(expiry_policies::power)),
            expiry_policies::refreshed.eq(excluded(expiry_policies::refreshed)),
            expiry_policies::own_deadline.eq(excluded(expiry_policies::own_deadline)),
            expiry_policies::size_bytes.eq(excluded(expiry_policies::size_bytes)),
        ))
        .execute(tx)?;
    Ok(())
}

fn file_expiry_change_required(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    expiry: FileExpiry,
) -> Result<bool, StoreError> {
    let current = load_expiry_policy_locked(tx, site_id, path)?;
    Ok(match expiry {
        FileExpiry::Preserve => false,
        FileExpiry::Clear => current.is_some(),
        FileExpiry::Policy(policy) => current.is_none_or(|stored| stored.policy != policy),
    })
}

fn apply_file_expiry(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    size: u64,
    expiry: FileExpiry,
    now: i64,
) -> Result<(), StoreError> {
    match expiry {
        FileExpiry::Preserve => {}
        FileExpiry::Clear => {
            diesel::delete(expiry_policies::table.find((site_id, path))).execute(tx)?;
        }
        FileExpiry::Policy(policy) => {
            store_expiry_policy_locked(
                tx,
                ExpiryPolicyWrite {
                    site_id,
                    path,
                    kind: ExpiryTargetKind::File,
                    policy,
                    size,
                    now,
                },
            )?;
        }
    }
    Ok(())
}

fn load_expiry_policy_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<Option<StoredExpiryPolicy>, StoreError> {
    let row = expiry_policies::table
        .find((site_id, path))
        .select(ExpiryPolicyRow::as_select())
        .first::<ExpiryPolicyRow>(db)
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let raw_kind = row.target_kind;
    let raw_mode = row.mode;
    let duration = row.duration_seconds;
    let deadline = row.deadline;
    let min_age = row.min_age_seconds;
    let max_age = row.max_age_seconds;
    let max_size = row.max_size_bytes;
    let power = row.power;
    let refreshed = row.refreshed;
    let own_deadline = row.own_deadline.ok_or(ExpiryError::InvalidTimestamp)?;
    let size = row.size_bytes;
    let mode = ExpiryMode::try_from(raw_mode)?;
    let policy = match mode {
        ExpiryMode::Relative => ExpiryPolicy::Relative {
            duration_seconds: duration
                .ok_or(ExpiryError::InvalidDuration)?
                .cast_unsigned(),
        },
        ExpiryMode::Absolute => ExpiryPolicy::Absolute {
            deadline_unix_seconds: deadline.ok_or(ExpiryError::InvalidTimestamp)?,
        },
        ExpiryMode::Decay => ExpiryPolicy::Decay(DecayPolicy {
            min_age_seconds: min_age.ok_or(ExpiryError::InvalidDuration)?.cast_unsigned(),
            max_age_seconds: max_age.ok_or(ExpiryError::InvalidDuration)?.cast_unsigned(),
            max_size_bytes: max_size.ok_or(ExpiryError::InvalidSize)?.cast_unsigned(),
            power: power.ok_or(ExpiryError::InvalidPower)?,
        }),
    };
    Ok(Some(StoredExpiryPolicy {
        kind: ExpiryTargetKind::try_from(raw_kind)?,
        policy,
        refreshed_millis: refreshed,
        own_deadline_millis: own_deadline,
        size_bytes: size.cast_unsigned(),
    }))
}

fn own_expiry_report(stored: StoredExpiryPolicy) -> OwnExpiryReport {
    let (min_age_seconds, max_age_seconds, max_size_bytes, power) = match stored.policy {
        ExpiryPolicy::Decay(policy) => (
            Some(policy.min_age_seconds),
            Some(policy.max_age_seconds),
            Some(policy.max_size_bytes),
            Some(policy.power),
        ),
        ExpiryPolicy::Relative { .. } | ExpiryPolicy::Absolute { .. } => (None, None, None, None),
    };
    OwnExpiryReport {
        mode: stored.policy.mode(),
        min_age_seconds,
        max_age_seconds,
        max_size_bytes,
        power,
        retention_seconds: stored
            .policy
            .retention_seconds(stored.size_bytes)
            .expect("stored expiry policy is valid"),
        expires_at: format_timestamp(stored.own_deadline_millis),
    }
}

struct ExpiryReportState {
    report: ExpiryReport,
    effective_millis: Option<i64>,
}

fn base_expiry_report_locked(
    db: &mut SqliteConnection,
    name: &str,
    site_id: i64,
    rel: &str,
    now: i64,
) -> Result<ExpiryReportState, StoreError> {
    let kind = expiry_target_kind_locked(db, name, rel)?;
    let size = expiry_target_size_locked(db, site_id, rel, kind)?;
    let own = load_expiry_policy_locked(db, site_id, rel)?;
    let mut inherited = Vec::new();
    for ancestor in expiry_ancestor_paths(rel) {
        if let Some(stored) = load_expiry_policy_locked(db, site_id, &ancestor)? {
            inherited.push((ancestor, stored));
        }
    }
    let own_policy = own.map(own_expiry_report);
    let mut effective = own.map(|stored| stored.own_deadline_millis);
    let mut limited_by = None;
    let inherited_caps = inherited
        .into_iter()
        .map(|(path, stored)| {
            if effective.is_none_or(|deadline| stored.own_deadline_millis < deadline) {
                effective = Some(stored.own_deadline_millis);
                limited_by = Some(ExpiryLimit {
                    kind: stored.kind,
                    path: (!path.is_empty()).then_some(path.clone()),
                });
            }
            InheritedExpiryCap {
                kind: stored.kind,
                path: (!path.is_empty()).then_some(path),
                expires_at: format_timestamp(stored.own_deadline_millis),
            }
        })
        .collect();
    Ok(ExpiryReportState {
        report: ExpiryReport {
            target: ExpiryTarget {
                site: name.to_string(),
                path: (!rel.is_empty()).then(|| rel.to_string()),
                kind,
            },
            size,
            refreshed_at: own
                .and_then(|stored| stored.refreshed_millis)
                .map(format_timestamp),
            own_policy,
            inherited_caps,
            effective_expires_at: effective.map(format_timestamp),
            remaining_seconds: effective
                .map(|deadline| remaining_seconds(deadline / 1000, now / 1000)),
            limited_by,
        },
        effective_millis: effective,
    })
}

fn effective_expiry_source(report: &ExpiryReport) -> ExpiryLimit {
    report.limited_by.clone().unwrap_or_else(|| ExpiryLimit {
        kind: report.target.kind,
        path: report.target.path.clone(),
    })
}

fn apply_resolved_expiry_cap(state: &mut ExpiryReportState, cap: &ExpiryReportState, now: i64) {
    let Some(deadline) = cap.effective_millis else {
        return;
    };
    let source = effective_expiry_source(&cap.report);
    let expires_at = format_timestamp(deadline);
    if !state.report.inherited_caps.iter().any(|existing| {
        existing.kind == source.kind
            && existing.path == source.path
            && existing.expires_at == expires_at
    }) {
        state.report.inherited_caps.push(InheritedExpiryCap {
            kind: source.kind,
            path: source.path.clone(),
            expires_at,
        });
    }
    if state
        .effective_millis
        .is_none_or(|current| deadline < current)
    {
        state.effective_millis = Some(deadline);
        state.report.effective_expires_at = Some(format_timestamp(deadline));
        state.report.remaining_seconds = Some(remaining_seconds(deadline / 1000, now / 1000));
        state.report.limited_by = Some(source);
    }
}

fn expiry_report_locked(
    db: &mut SqliteConnection,
    name: &str,
    site_id: i64,
    rel: &str,
    now: i64,
) -> Result<ExpiryReport, StoreError> {
    let mut state = base_expiry_report_locked(db, name, site_id, rel, now)?;
    let (resolution, physical, dependencies) = resolve_db_path_trace(db, site_id, rel)?;
    let mut capped_paths = BTreeSet::new();
    for dependency in dependencies {
        if dependency == rel || !capped_paths.insert(dependency.clone()) {
            continue;
        }
        let cap = base_expiry_report_locked(db, name, site_id, &dependency, now)?;
        apply_resolved_expiry_cap(&mut state, &cap, now);
    }
    if !matches!(resolution, GraphResolution::Missing)
        && physical != rel
        && capped_paths.insert(physical.clone())
    {
        let cap = base_expiry_report_locked(db, name, site_id, &physical, now)?;
        apply_resolved_expiry_cap(&mut state, &cap, now);
    }
    Ok(state.report)
}

fn expiry_ancestor_paths(rel: &str) -> Vec<String> {
    if rel.is_empty() {
        return Vec::new();
    }
    let mut ancestors = Vec::new();
    let mut current = rel;
    while let Some((parent, _)) = current.rsplit_once('/') {
        ancestors.push(parent.to_string());
        current = parent;
    }
    ancestors.push(String::new());
    ancestors
}

fn policy_is_affected(path: &str, kind: ExpiryTargetKind, changed: &str) -> bool {
    match kind {
        ExpiryTargetKind::Site => true,
        ExpiryTargetKind::File => path == changed,
        ExpiryTargetKind::Folder => {
            changed == path
                || changed
                    .strip_prefix(path)
                    .is_some_and(|tail| tail.starts_with('/'))
        }
    }
}

fn copy_expiry_policies_locked(
    tx: &mut SqliteConnection,
    source_id: i64,
    destination_id: i64,
    now: i64,
) -> Result<(), StoreError> {
    let paths = expiry_policies::table
        .filter(expiry_policies::site_id.eq(source_id))
        .select(expiry_policies::path)
        .order(expiry_policies::path)
        .load::<String>(tx)?;
    for path in paths {
        let stored = load_expiry_policy_locked(tx, source_id, &path)?
            .expect("selected expiry policy still exists");
        let size = expiry_target_size_locked(tx, destination_id, &path, stored.kind)?;
        store_expiry_policy_locked(
            tx,
            ExpiryPolicyWrite {
                site_id: destination_id,
                path: &path,
                kind: stored.kind,
                policy: stored.policy,
                size,
                now,
            },
        )?;
    }
    Ok(())
}

fn refresh_expiry_for_changes_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    changed_paths: &[&str],
    now: i64,
) -> Result<(), StoreError> {
    let policies = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select((expiry_policies::path, expiry_policies::target_kind))
        .load::<(String, i64)>(tx)?;
    if policies.is_empty() {
        return Ok(());
    }
    let policy_paths = policies.iter().map(|(path, _)| path).collect::<Vec<_>>();
    let mut alias_kinds = HashMap::new();
    for chunk in policy_paths.chunks(30_000) {
        alias_kinds.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.eq_any(chunk))
                .select((aliases::path, aliases::resolved_kind))
                .load::<(String, Option<i64>)>(tx)?,
        );
    }
    for (path, raw_kind) in policies {
        let stored_kind = ExpiryTargetKind::try_from(raw_kind)?;
        let directly_affected = changed_paths
            .iter()
            .any(|changed| policy_is_affected(&path, stored_kind, changed));
        let alias_dependency_affected = if alias_kinds.contains_key(&path) {
            let (_, target, dependencies) = resolve_db_path_trace(tx, site_id, &path)?;
            changed_paths.iter().any(|changed| {
                dependencies.iter().any(|dependency| *changed == dependency)
                    || *changed == target
                    || changed
                        .strip_prefix(&target)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                    || target
                        .strip_prefix(*changed)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        } else {
            false
        };
        if !directly_affected && !alias_dependency_affected {
            continue;
        }
        let Some(stored) = load_expiry_policy_locked(tx, site_id, &path)? else {
            continue;
        };
        let kind = match alias_kinds.get(&path) {
            Some(Some(kind)) if *kind == AliasResolvedKind::File as i64 => ExpiryTargetKind::File,
            Some(Some(kind)) if *kind == AliasResolvedKind::Directory as i64 => {
                ExpiryTargetKind::Folder
            }
            Some(None) | None => stored_kind,
            Some(Some(_)) => return Err(StoreError::InvalidAliasTarget),
        };
        let size = expiry_target_size_locked(tx, site_id, &path, kind)?;
        store_expiry_policy_locked(
            tx,
            ExpiryPolicyWrite {
                site_id,
                path: &path,
                kind,
                policy: stored.policy,
                size,
                now,
            },
        )?;
    }
    Ok(())
}

fn finish_partial_expiry_locked(
    tx: &mut SqliteConnection,
    blobs: &BlobFiles,
    site_id: i64,
    changed_path: &str,
    now: i64,
) -> Result<(), StoreError> {
    let remaining = site_entries::table
        .filter(site_entries::site_id.eq(site_id))
        .filter(site_entries::path.ne(MANIFEST_PATH))
        .filter(
            site_entries::kind
                .eq(database::schema::FILE_ENTRY_KIND)
                .or(site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND))
                .or(site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND)),
        )
        .select(count_star())
        .first::<i64>(tx)?;
    if remaining == 0 {
        let name = sites::table
            .find(site_id)
            .select(sites::name)
            .first::<String>(tx)?;
        retain_management_tombstone(tx, &name, now)?;
        diesel::delete(sites::table.find(site_id)).execute(tx)?;
        return Ok(());
    }
    diesel::update(sites::table.find(site_id))
        .set(sites::content_revision.eq(sites::content_revision + 1))
        .execute(tx)?;
    refresh_aliases_locked(tx, site_id, &[AliasChange::Subtree(changed_path)])?;
    refresh_expiry_for_changes_locked(tx, site_id, &[changed_path], now)?;
    regenerate_site(tx, blobs, site_id, now)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn regenerate_site(
    tx: &mut SqliteConnection,
    blobs: &BlobFiles,
    site_id: i64,
    updated: i64,
) -> Result<String, StoreError> {
    let (name, public_url, revision, management_status) = sites::table
        .find(site_id)
        .select((
            sites::name,
            sites::public_url,
            sites::content_revision,
            sites::management_status,
        ))
        .first::<(String, String, i64, i64)>(tx)?;
    let managed = management_status != 0;
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::hash))
        .order(files::path)
        .load::<(String, String)>(tx)?;
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((allocated_entries::path, allocated_entries::hash))
        .load::<(String, String)>(tx)?;
    for (path, hash) in allocated {
        entries.push((path, hash));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in &entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(hash.as_bytes());
    }
    let alias_entries = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select((aliases::path, aliases::canonical_target))
        .order(aliases::path)
        .load::<(String, String)>(tx)?;
    for (path, target) in &alias_entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(target.len() as u64).to_le_bytes());
        hasher.update(target.as_bytes());
    }
    let tree_hash = format!("blake3:{}", hasher.finalize().to_hex());
    let mut manifest = format!(
        "version = 1\nhost = \"{}\"\nname = \"{}\"\nmanaged = {managed}\ncontent_revision = {}\ntree_hash = \"{}\"\n\n[files]\n",
        toml_escape(&public_url),
        toml_escape(&name),
        revision,
        tree_hash
    );
    for (path, hash) in &entries {
        writeln!(manifest, "\"{}\" = \"blake3:{}\"", toml_escape(path), hash)
            .expect("writing to String cannot fail");
    }
    if !alias_entries.is_empty() {
        manifest.push_str("\n[aliases]\n");
        for (path, target) in &alias_entries {
            writeln!(
                manifest,
                "\"{}\" = \"{}\"",
                toml_escape(path),
                toml_escape(target)
            )
            .expect("writing to String cannot fail");
        }
    }
    let expiry_paths = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select(expiry_policies::path)
        .order((expiry_policies::target_kind, expiry_policies::path))
        .load::<String>(tx)?;
    for path in expiry_paths {
        let stored = load_expiry_policy_locked(tx, site_id, &path)?
            .expect("selected expiry policy still exists");
        let table = match stored.kind {
            ExpiryTargetKind::Site => "[expiry.site]".to_string(),
            ExpiryTargetKind::Folder => {
                format!("[expiry.folders.\"{}\"]", toml_escape(&path))
            }
            ExpiryTargetKind::File => format!("[expiry.files.\"{}\"]", toml_escape(&path)),
        };
        write!(
            manifest,
            "\n{table}\nmode = \"{}\"\nexpires_at = \"{}\"\n",
            expiry_mode_name(stored.policy.mode()),
            format_timestamp(stored.own_deadline_millis),
        )
        .expect("writing to String cannot fail");
        match stored.policy {
            ExpiryPolicy::Relative { duration_seconds } => {
                writeln!(manifest, "duration_seconds = {duration_seconds}")
                    .expect("writing to String cannot fail");
            }
            ExpiryPolicy::Absolute { .. } => {}
            ExpiryPolicy::Decay(policy) => {
                write!(
                    manifest,
                    "min_age_seconds = {}\nmax_age_seconds = {}\nmax_size_bytes = {}\npower = {}\n",
                    policy.min_age_seconds,
                    policy.max_age_seconds,
                    policy.max_size_bytes,
                    policy.power,
                )
                .expect("writing to String cannot fail");
            }
        }
    }
    let staged = stage_bytes(MANIFEST_PATH, manifest.as_bytes());
    blobs.put_bytes(&staged.hash, manifest.as_bytes())?;
    diesel::insert_into(blobs::table)
        .values((
            blobs::hash.eq(&staged.hash),
            blobs::bytes.eq(Vec::<u8>::new()),
            blobs::size.eq(staged.size),
        ))
        .on_conflict_do_nothing()
        .execute(tx)?;
    ensure_file_entry(tx, site_id, MANIFEST_PATH)?;
    diesel::insert_into(files::table)
        .values(NewFile {
            site_id,
            path: MANIFEST_PATH,
            hash: &staged.hash,
            size: staged.size,
        })
        .on_conflict((files::site_id, files::path))
        .do_update()
        .set((
            files::hash.eq(excluded(files::hash)),
            files::size.eq(excluded(files::size)),
        ))
        .execute(tx)?;
    diesel::update(sites::table.find(site_id))
        .set((sites::tree_hash.eq(&tree_hash), sites::updated.eq(updated)))
        .execute(tx)?;
    Ok(tree_hash)
}

const fn expiry_mode_name(mode: ExpiryMode) -> &'static str {
    match mode {
        ExpiryMode::Relative => "relative",
        ExpiryMode::Absolute => "absolute",
        ExpiryMode::Decay => "decay",
    }
}

fn site_revision_locked(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<(u64, String), StoreError> {
    let (revision, tree_hash) = sites::table
        .filter(sites::name.eq(name))
        .select((sites::content_revision, sites::tree_hash))
        .first::<(i64, String)>(db)
        .map_err(map_sql)?;
    Ok((revision.cast_unsigned(), tree_hash))
}

fn retain_management_tombstone(
    tx: &mut SqliteConnection,
    name: &str,
    now: i64,
) -> Result<(), diesel::result::Error> {
    let hash = sites::table
        .filter(sites::name.eq(name))
        .filter(sites::management_status.eq(1_i64))
        .select(sites::management_hash)
        .first::<Option<Vec<u8>>>(tx)
        .optional()?
        .flatten();
    if let Some(hash) = hash {
        diesel::insert_into(management_tombstones::table)
            .values((
                management_tombstones::name.eq(name),
                management_tombstones::management_hash.eq(hash),
                management_tombstones::created.eq(now),
            ))
            .on_conflict(management_tombstones::name)
            .do_update()
            .set((
                management_tombstones::management_hash
                    .eq(excluded(management_tombstones::management_hash)),
                management_tombstones::created.eq(excluded(management_tombstones::created)),
            ))
            .execute(tx)?;
    }
    Ok(())
}

fn prune_undo_locked(tx: &mut SqliteConnection, now: i64) -> Result<(), diesel::result::Error> {
    diesel::delete(
        undo_operations::table.filter(
            undo_operations::consumed
                .eq(1_i64)
                .or(undo_operations::expires.le(now)),
        ),
    )
    .execute(tx)?;
    let rows = undo_names::table
        .inner_join(undo_operations::table.on(undo_operations::token.eq(undo_names::token)))
        .select((undo_names::name, undo_names::token))
        .order((
            undo_names::name,
            undo_operations::created.desc(),
            undo_operations::rowid.desc(),
        ))
        .load::<(String, String)>(tx)?;
    let mut previous = None::<String>;
    let mut position = 0_i64;
    let mut stale = Vec::new();
    for (name, token) in rows {
        if previous.as_deref() == Some(name.as_str()) {
            position += 1;
        } else {
            previous = Some(name);
            position = 1;
        }
        if position > UNDO_LIMIT_PER_SITE {
            stale.push(token);
        }
    }
    stale.sort_unstable();
    stale.dedup();
    for tokens in stale.chunks(SQLITE_DELETE_BATCH_SIZE) {
        diesel::delete(undo_operations::table.filter(undo_operations::token.eq_any(tokens)))
            .execute(tx)?;
    }
    Ok(())
}

fn undo_token() -> Result<String, StoreError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(token, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(token)
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn format_timestamp(millis: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .expect("timestamp is representable")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting succeeds")
}

#[derive(PartialEq, Eq)]
enum NodeKind {
    Missing,
    Dir,
    File { hash: String },
}

#[derive(Clone)]
struct RealEntry {
    hash: String,
    size: i64,
}

enum GraphResolution {
    Missing,
    Directory,
    File(RealEntry),
}

#[derive(Clone, Copy)]
enum AliasChange<'a> {
    Entry(&'a str),
    Alias(&'a str),
    Subtree(&'a str),
}

impl<'a> AliasChange<'a> {
    const fn path(self) -> &'a str {
        match self {
            Self::Entry(path) | Self::Alias(path) | Self::Subtree(path) => path,
        }
    }

    const fn includes_target_descendants(self) -> bool {
        matches!(self, Self::Alias(_) | Self::Subtree(_))
    }
}

fn load_real_entries(
    db: &mut SqliteConnection,
    site_id: i64,
) -> Result<BTreeMap<String, RealEntry>, StoreError> {
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::hash, files::size))
        .load::<(String, String, i64)>(db)?
        .into_iter()
        .map(|(path, hash, size)| (path, RealEntry { hash, size }))
        .collect::<BTreeMap<_, _>>();
    entries.extend(
        allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
            ))
            .load::<(String, String, i64)>(db)?
            .into_iter()
            .map(|(path, hash, size)| (path, RealEntry { hash, size })),
    );
    Ok(entries)
}

fn path_has_descendant<T>(entries: &BTreeMap<String, T>, path: &str) -> bool {
    let (start, end) = descendant_bounds(path);
    entries.range(start..end).next().is_some()
}

fn alias_substitution<'a>(
    aliases: &'a BTreeMap<String, String>,
    path: &str,
) -> Option<(&'a str, &'a str)> {
    if let Some((path, target)) = aliases.get_key_value(path) {
        return Some((path, target));
    }
    let mut end = path.len();
    while let Some(slash) = path[..end].rfind('/') {
        let prefix = &path[..slash];
        if let Some((prefix, target)) = aliases.get_key_value(prefix) {
            return Some((prefix, target));
        }
        end = slash;
    }
    None
}

fn resolve_graph_path(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<GraphResolution, StoreError> {
    resolve_graph_path_final(real, aliases, initial).map(|(resolution, _)| resolution)
}

fn resolve_graph_path_final(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<(GraphResolution, String), StoreError> {
    resolve_graph_path_trace(real, aliases, initial).map(|(resolution, path, _)| (resolution, path))
}

fn resolve_graph_path_trace(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<(GraphResolution, String, Vec<String>), StoreError> {
    let mut path = initial.to_string();
    let mut visited = HashSet::new();
    let mut dependencies = Vec::new();
    for _ in 0..MAX_ALIAS_HOPS {
        if !visited.insert(path.clone()) {
            return Err(StoreError::AliasCycle);
        }
        if let Some(entry) = real.get(&path) {
            return Ok((GraphResolution::File(entry.clone()), path, dependencies));
        }
        if let Some((prefix, target)) = alias_substitution(aliases, &path) {
            dependencies.push(prefix.to_string());
            let suffix = &path[prefix.len()..];
            path = format!("{target}{suffix}");
            continue;
        }
        if path_has_descendant(real, &path) || path_has_descendant(aliases, &path) {
            return Ok((GraphResolution::Directory, path, dependencies));
        }
        return Ok((GraphResolution::Missing, path, dependencies));
    }
    Err(StoreError::AliasHopLimit)
}

fn load_alias_rows_at_paths(
    tx: &mut SqliteConnection,
    site_id: i64,
    paths: &BTreeSet<String>,
) -> Result<Vec<AliasRow>, StoreError> {
    let mut rows = Vec::new();
    for chunk in paths.iter().collect::<Vec<_>>().chunks(30_000) {
        rows.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.eq_any(chunk))
                .select(AliasRow::as_select())
                .load::<AliasRow>(tx)?,
        );
    }
    Ok(rows)
}

fn load_alias_rows_for_exact_targets(
    tx: &mut SqliteConnection,
    site_id: i64,
    targets: &BTreeSet<String>,
) -> Result<Vec<AliasRow>, StoreError> {
    let mut rows = Vec::new();
    for chunk in targets.iter().collect::<Vec<_>>().chunks(30_000) {
        rows.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::canonical_target.eq_any(chunk))
                .select(AliasRow::as_select())
                .load::<AliasRow>(tx)?,
        );
    }
    Ok(rows)
}

fn alias_dependency_targets(paths: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for path in paths {
        if path.is_empty() {
            continue;
        }
        targets.insert(path.clone());
        targets.extend(
            aggregate_paths(&path)
                .into_iter()
                .filter(|ancestor| !ancestor.is_empty())
                .map(str::to_string),
        );
    }
    targets
}

fn add_affected_alias_rows(
    affected: &mut BTreeMap<String, AliasRow>,
    pending: &mut Vec<String>,
    rows: Vec<AliasRow>,
) {
    record_alias_refresh_rows(0, rows.len());
    for row in rows {
        if affected.contains_key(&row.path) {
            continue;
        }
        pending.push(row.path.clone());
        affected.insert(row.path.clone(), row);
    }
}

fn refresh_aliases_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    changes: &[AliasChange<'_>],
) -> Result<(), StoreError> {
    const FULL_REFRESH_THRESHOLD: usize = 512;

    if changes.is_empty() {
        return Ok(());
    }
    let changed_paths = changes
        .iter()
        .map(|change| change.path().to_string())
        .collect::<BTreeSet<_>>();
    let mut affected = BTreeMap::new();
    let mut pending = changes
        .iter()
        .filter(|change| change.includes_target_descendants())
        .map(|change| change.path().to_string())
        .collect::<Vec<_>>();
    add_affected_alias_rows(
        &mut affected,
        &mut pending,
        load_alias_rows_at_paths(tx, site_id, &changed_paths)?,
    );
    let dependency_targets = alias_dependency_targets(changed_paths.iter().cloned());
    add_affected_alias_rows(
        &mut affected,
        &mut pending,
        load_alias_rows_for_exact_targets(tx, site_id, &dependency_targets)?,
    );
    if affected.len() > FULL_REFRESH_THRESHOLD {
        return refresh_all_aliases_locked(tx, site_id);
    }

    let mut expanded = BTreeSet::new();
    let mut cursor = 0;
    while cursor < pending.len() {
        let path = pending[cursor].clone();
        cursor += 1;
        if path.is_empty() || !expanded.insert(path.clone()) {
            continue;
        }
        let targets = alias_dependency_targets(std::iter::once(path.clone()));
        add_affected_alias_rows(
            &mut affected,
            &mut pending,
            load_alias_rows_for_exact_targets(tx, site_id, &targets)?,
        );
        let (start, end) = descendant_bounds(&path);
        let descendants = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::canonical_target.ge(start))
            .filter(aliases::canonical_target.lt(end))
            .select(AliasRow::as_select())
            .load::<AliasRow>(tx)?;
        add_affected_alias_rows(&mut affected, &mut pending, descendants);
        if affected.len() > FULL_REFRESH_THRESHOLD {
            return refresh_all_aliases_locked(tx, site_id);
        }
    }

    for row in affected.into_values() {
        let (resolution, final_target, _) =
            resolve_db_path_final(tx, site_id, &row.canonical_target)?;
        if matches!(resolution, GraphResolution::Directory)
            && row
                .path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        let value = match resolution {
            GraphResolution::Missing => (None, None, None),
            GraphResolution::Directory => (Some(AliasResolvedKind::Directory as i64), None, None),
            GraphResolution::File(file) => (
                Some(AliasResolvedKind::File as i64),
                Some(file.hash),
                Some(file.size),
            ),
        };
        resolve_db_path_final(tx, site_id, &row.path)?;
        if (
            row.resolved_kind,
            row.resolved_hash.as_ref(),
            row.resolved_size,
        ) != (value.0, value.1.as_ref(), value.2)
        {
            diesel::update(aliases::table.find((site_id, row.path.as_str())))
                .set((
                    aliases::resolved_kind.eq(value.0),
                    aliases::resolved_hash.eq(value.1),
                    aliases::resolved_size.eq(value.2),
                ))
                .execute(tx)?;
        }
    }
    Ok(())
}

fn refresh_all_aliases_locked(tx: &mut SqliteConnection, site_id: i64) -> Result<(), StoreError> {
    let real = load_real_entries(tx, site_id)?;
    let rows = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(tx)?;
    record_alias_refresh_rows(real.len(), rows.len());
    let graph = rows
        .iter()
        .map(|row| (row.path.clone(), row.canonical_target.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = Vec::with_capacity(rows.len());
    for row in rows {
        let (resolution, final_target) =
            resolve_graph_path_final(&real, &graph, &row.canonical_target)?;
        if matches!(resolution, GraphResolution::Directory)
            && row
                .path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        let value = match resolution {
            GraphResolution::Missing => (None, None, None),
            GraphResolution::Directory => (Some(AliasResolvedKind::Directory as i64), None, None),
            GraphResolution::File(file) => (
                Some(AliasResolvedKind::File as i64),
                Some(file.hash),
                Some(file.size),
            ),
        };
        // Resolving the alias path itself catches prefix-substitution cycles that
        // are not visible by following only its canonical target.
        resolve_graph_path(&real, &graph, &row.path)?;
        if (
            row.resolved_kind,
            row.resolved_hash.as_ref(),
            row.resolved_size,
        ) != (value.0, value.1.as_ref(), value.2)
        {
            resolved.push((row.path, value));
        }
    }
    for (path, (kind, hash, size)) in resolved {
        diesel::update(aliases::table.find((site_id, path.as_str())))
            .set((
                aliases::resolved_kind.eq(kind),
                aliases::resolved_hash.eq(hash),
                aliases::resolved_size.eq(size),
            ))
            .execute(tx)?;
    }
    Ok(())
}

fn validate_alias_graph_with_staged(
    tx: &mut SqliteConnection,
    site_id: i64,
    staged: &[&StagedFile],
    archive_aliases: &[ArchiveAlias<'_>],
) -> Result<(), StoreError> {
    let mut real = load_real_entries(tx, site_id)?;
    for file in staged {
        real.insert(
            file.path.clone(),
            RealEntry {
                hash: file.hash.clone(),
                size: file.size,
            },
        );
    }
    let mut graph = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select((aliases::path, aliases::canonical_target))
        .load::<(String, String)>(tx)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    graph.extend(
        archive_aliases
            .iter()
            .map(|alias| (alias.path.to_string(), alias.target.to_string())),
    );
    for (path, target) in &graph {
        let (resolution, final_target) = resolve_graph_path_final(&real, &graph, target)?;
        if matches!(resolution, GraphResolution::Directory)
            && path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        resolve_graph_path(&real, &graph, path)?;
    }
    Ok(())
}

fn validate_archive_alias_conflicts_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    archive_aliases: &[ArchiveAlias<'_>],
) -> Result<(), StoreError> {
    let entries = site_entries::table
        .filter(site_entries::site_id.eq(site_id))
        .select((site_entries::path, site_entries::kind))
        .load::<(String, i64)>(tx)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    for alias in archive_aliases {
        if aggregate_paths(alias.path).into_iter().any(|ancestor| {
            !ancestor.is_empty()
                && entries.get(ancestor).is_some_and(|kind| {
                    *kind == database::schema::ALIAS_ENTRY_KIND || ancestor != alias.path
                })
        }) {
            return Err(StoreError::AliasWrite);
        }
        if entries
            .get(alias.path)
            .is_some_and(|kind| *kind != database::schema::ALIAS_ENTRY_KIND)
        {
            return Err(StoreError::AliasConflict);
        }
        let (start, end) = descendant_bounds(alias.path);
        if entries.range(start..end).next().is_some() {
            return Err(StoreError::AliasConflict);
        }
    }
    Ok(())
}

fn reject_alias_write_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    allow_exact: bool,
) -> Result<(), StoreError> {
    reject_alias_writes_locked(db, site_id, [path], allow_exact)
}

fn reject_expiry_below_alias_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<(), StoreError> {
    let ancestors = aggregate_paths(path)
        .into_iter()
        .filter(|ancestor| !ancestor.is_empty())
        .collect::<Vec<_>>();
    if ancestors.is_empty() {
        return Ok(());
    }
    let found = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .filter(aliases::path.eq_any(ancestors))
        .select(aliases::site_id)
        .first::<i64>(db)
        .optional()?
        .is_some();
    if found {
        Err(StoreError::AliasWrite)
    } else {
        Ok(())
    }
}

fn reject_alias_writes_locked<'a>(
    db: &mut SqliteConnection,
    site_id: i64,
    paths: impl IntoIterator<Item = &'a str>,
    allow_exact: bool,
) -> Result<(), StoreError> {
    let paths = paths.into_iter().collect::<Vec<_>>();
    let mut exact_conflicts = paths
        .iter()
        .flat_map(|path| aggregate_paths(path))
        .filter(|ancestor| !ancestor.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if !allow_exact {
        exact_conflicts.extend(paths.iter().map(|path| (*path).to_string()));
    }
    for chunk in exact_conflicts.iter().collect::<Vec<_>>().chunks(30_000) {
        let found = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(chunk))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some();
        if found {
            return Err(StoreError::AliasWrite);
        }
    }
    if allow_exact {
        return Ok(());
    }
    if paths.len() <= 512 {
        for path in paths {
            let (start, end) = descendant_bounds(path);
            let found = aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.ge(start))
                .filter(aliases::path.lt(end))
                .select(aliases::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            if found {
                return Err(StoreError::AliasWrite);
            }
        }
        return Ok(());
    }

    let changed = paths
        .iter()
        .map(|path| (*path).to_string())
        .collect::<BTreeSet<_>>();
    let top_levels = paths
        .iter()
        .filter_map(|path| path.split('/').next())
        .collect::<BTreeSet<_>>();
    for top_level in top_levels {
        let (start, end) = descendant_bounds(top_level);
        let candidates = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.ge(start))
            .filter(aliases::path.lt(end))
            .select(aliases::path)
            .load::<String>(db)?;
        if candidates.into_iter().any(|candidate| {
            aggregate_paths(&candidate)
                .into_iter()
                .filter(|ancestor| !ancestor.is_empty())
                .any(|ancestor| changed.contains(ancestor))
        }) {
            return Err(StoreError::AliasWrite);
        }
    }
    Ok(())
}

fn site_exists_locked(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<bool, diesel::result::Error> {
    sites::table
        .filter(sites::name.eq(name))
        .select(sites::id)
        .first::<i64>(db)
        .optional()
        .map(|site| site.is_some())
}

fn site_id_locked(db: &mut SqliteConnection, name: &str) -> Result<i64, StoreError> {
    sites::table
        .filter(sites::name.eq(name))
        .select(sites::id)
        .first::<i64>(db)
        .map_err(map_sql)
}

fn node_locked(db: &mut SqliteConnection, name: &str, rel: &str) -> Result<NodeKind, StoreError> {
    if rel.is_empty() {
        return site_exists_locked(db, name)
            .map(|exists| {
                if exists {
                    NodeKind::Dir
                } else {
                    NodeKind::Missing
                }
            })
            .map_err(StoreError::Sqlite);
    }
    let Some(site_id) = sites::table
        .filter(sites::name.eq(name))
        .select(sites::id)
        .first::<i64>(db)
        .optional()?
    else {
        return Ok(NodeKind::Missing);
    };
    let hash = files::table
        .find((site_id, rel))
        .select(files::hash)
        .first::<String>(db)
        .optional()?;
    if let Some(hash) = hash {
        return Ok(NodeKind::File { hash });
    }
    let allocated = allocated_entries::table
        .find((site_id, rel))
        .select(allocated_entries::hash)
        .first::<String>(db)
        .optional()?;
    if let Some(hash) = allocated {
        return Ok(NodeKind::File { hash });
    }
    if let Some(node) = alias_node_locked(db, site_id, rel)? {
        return Ok(node);
    }
    let (prefix_start, prefix_end) = descendant_bounds(rel);
    let regular_dir_exists = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ge(&prefix_start))
        .filter(files::path.lt(&prefix_end))
        .select(files::site_id)
        .first::<i64>(db)
        .optional()?
        .is_some();
    let allocated_dir_exists = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .filter(allocated_entries::path.ge(&prefix_start))
        .filter(allocated_entries::path.lt(&prefix_end))
        .select(allocated_entries::site_id)
        .first::<i64>(db)
        .optional()?
        .is_some();
    let alias_dir_exists = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .filter(aliases::path.ge(&prefix_start))
        .filter(aliases::path.lt(&prefix_end))
        .select(aliases::site_id)
        .first::<i64>(db)
        .optional()?
        .is_some();
    Ok(
        if regular_dir_exists || allocated_dir_exists || alias_dir_exists {
            NodeKind::Dir
        } else {
            NodeKind::Missing
        },
    )
}

fn alias_node_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    rel: &str,
) -> Result<Option<NodeKind>, StoreError> {
    let direct_alias = aliases::table
        .find((site_id, rel))
        .select((aliases::resolved_kind, aliases::resolved_hash))
        .first::<(Option<i64>, Option<String>)>(db)
        .optional()?;
    if let Some((kind, hash)) = direct_alias {
        return Ok(Some(match (kind, hash) {
            (Some(kind), Some(hash)) if kind == AliasResolvedKind::File as i64 => {
                NodeKind::File { hash }
            }
            (Some(kind), _) if kind == AliasResolvedKind::Directory as i64 => NodeKind::Dir,
            _ => NodeKind::Missing,
        }));
    }
    let ancestor_paths = aggregate_paths(rel)
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    let has_alias_ancestor = if ancestor_paths.is_empty() {
        false
    } else {
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(&ancestor_paths))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some()
    };
    if has_alias_ancestor {
        let candidate_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?;
        let graph = candidate_aliases.into_iter().collect::<BTreeMap<_, _>>();
        let real_entries = load_real_entries(db, site_id)?;
        return Ok(Some(
            match resolve_graph_path(&real_entries, &graph, rel)? {
                GraphResolution::Missing => NodeKind::Missing,
                GraphResolution::Directory => NodeKind::Dir,
                GraphResolution::File(file) => NodeKind::File { hash: file.hash },
            },
        ));
    }
    Ok(None)
}

fn resolved_alias_directory_target_locked(
    db: &mut SqliteConnection,
    name: &str,
    rel: &str,
) -> Result<Option<String>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let (resolved, target, used_alias) = resolve_db_path_final(db, site_id, rel)?;
    match resolved {
        GraphResolution::Directory if used_alias => Ok(Some(target)),
        GraphResolution::Missing | GraphResolution::File(_) | GraphResolution::Directory => {
            Ok(None)
        }
    }
}

fn resolve_db_path_final(
    db: &mut SqliteConnection,
    site_id: i64,
    initial: &str,
) -> Result<(GraphResolution, String, bool), StoreError> {
    resolve_db_path_trace(db, site_id, initial)
        .map(|(resolution, path, dependencies)| (resolution, path, !dependencies.is_empty()))
}

fn resolve_db_path_trace(
    db: &mut SqliteConnection,
    site_id: i64,
    initial: &str,
) -> Result<(GraphResolution, String, Vec<String>), StoreError> {
    let mut path = initial.to_string();
    let mut visited = HashSet::new();
    let mut dependencies = Vec::new();
    for _ in 0..MAX_ALIAS_HOPS {
        if !visited.insert(path.clone()) {
            return Err(StoreError::AliasCycle);
        }
        let file = files::table
            .find((site_id, path.as_str()))
            .select((files::hash, files::size))
            .first::<(String, i64)>(db)
            .optional()?;
        record_alias_resolution_rows(usize::from(file.is_some()));
        if let Some((hash, size)) = file {
            return Ok((
                GraphResolution::File(RealEntry { hash, size }),
                path,
                dependencies,
            ));
        }
        let allocated = allocated_entries::table
            .find((site_id, path.as_str()))
            .select((allocated_entries::hash, allocated_entries::size))
            .first::<(String, i64)>(db)
            .optional()?;
        record_alias_resolution_rows(usize::from(allocated.is_some()));
        if let Some((hash, size)) = allocated {
            return Ok((
                GraphResolution::File(RealEntry { hash, size }),
                path,
                dependencies,
            ));
        }
        let mut candidates = aggregate_paths(&path)
            .into_iter()
            .filter(|candidate| !candidate.is_empty())
            .collect::<Vec<_>>();
        candidates.push(path.as_str());
        candidates.sort_unstable();
        candidates.dedup();
        let substitutions = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(candidates))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?;
        record_alias_resolution_rows(substitutions.len());
        if let Some((prefix, target)) = substitutions
            .into_iter()
            .max_by_key(|(prefix, _)| prefix.len())
        {
            let suffix = &path[prefix.len()..];
            dependencies.push(prefix.clone());
            path = format!("{target}{suffix}");
            continue;
        }
        let (start, end) = descendant_bounds(&path);
        let file_descendant = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ge(&start))
            .filter(files::path.lt(&end))
            .select(files::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some();
        record_alias_resolution_rows(usize::from(file_descendant));
        let allocated_descendant = if file_descendant {
            false
        } else {
            let found = allocated_entries::table
                .filter(allocated_entries::site_id.eq(site_id))
                .filter(allocated_entries::path.ge(&start))
                .filter(allocated_entries::path.lt(&end))
                .select(allocated_entries::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            record_alias_resolution_rows(usize::from(found));
            found
        };
        let alias_descendant = if file_descendant || allocated_descendant {
            false
        } else {
            let found = aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.ge(&start))
                .filter(aliases::path.lt(&end))
                .select(aliases::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            record_alias_resolution_rows(usize::from(found));
            found
        };
        if file_descendant || allocated_descendant || alias_descendant {
            return Ok((GraphResolution::Directory, path, dependencies));
        }
        return Ok((GraphResolution::Missing, path, dependencies));
    }
    Err(StoreError::AliasHopLimit)
}

fn load_alias_directory_files(
    db: &mut SqliteConnection,
    name: &str,
    logical: &str,
    target: &str,
) -> Result<Vec<(String, u64)>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let (start, end) = descendant_bounds(target);
    let mut rows = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ge(&start))
        .filter(files::path.lt(&end))
        .select((files::path, files::size))
        .load::<(String, i64)>(db)?;
    record_alias_listed_file_rows(rows.len());
    let allocated_rows = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .filter(allocated_entries::path.ge(&start))
        .filter(allocated_entries::path.lt(&end))
        .select((allocated_entries::path, allocated_entries::size))
        .load::<(String, i64)>(db)?;
    record_alias_listed_allocated_rows(allocated_rows.len());
    rows.extend(allocated_rows);
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(rows
        .into_iter()
        .map(|(path, size)| {
            (
                format!("{logical}{}", &path[target.len()..]),
                size.cast_unsigned(),
            )
        })
        .collect())
}

fn load_directory_aliases(
    db: &mut SqliteConnection,
    name: &str,
    logical: &str,
    physical: &str,
) -> Result<Vec<AliasEntry>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let mut query = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .into_boxed();
    if !physical.is_empty() {
        let (start, end) = descendant_bounds(physical);
        query = query
            .filter(aliases::path.ge(start))
            .filter(aliases::path.lt(end));
    }
    let rows = query
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(db)?;
    record_alias_listed_rows(rows.len());
    let mut aggregate_targets = BTreeSet::new();
    let mut prepared = Vec::with_capacity(rows.len());
    for row in rows {
        let physical_alias = row.path.clone();
        let Some(suffix) = (if physical.is_empty() {
            Some(row.path.clone())
        } else {
            row.path
                .strip_prefix(physical)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .map(str::to_string)
        }) else {
            continue;
        };
        let aggregate_target = if row.resolved_kind == Some(AliasResolvedKind::Directory as i64) {
            resolved_alias_directory_target_locked(db, name, &physical_alias)?.inspect(|target| {
                aggregate_targets.insert(target.clone());
            })
        } else {
            None
        };
        prepared.push((row, suffix, aggregate_target));
    }
    let mut aggregate_counts = BTreeMap::new();
    let aggregate_targets = aggregate_targets.into_iter().collect::<Vec<_>>();
    for targets in aggregate_targets.chunks(500) {
        let loaded = path_aggregates::table
            .filter(path_aggregates::site_id.eq(site_id))
            .filter(path_aggregates::path.eq_any(targets))
            .select((path_aggregates::path, path_aggregates::file_count))
            .load::<(String, i64)>(db)?;
        record_alias_aggregate_rows(loaded.len());
        aggregate_counts.extend(loaded);
    }
    let mut entries = Vec::with_capacity(prepared.len());
    for (mut row, suffix, aggregate_target) in prepared {
        let resolved_files = aggregate_target.map(|target| {
            aggregate_counts
                .get(&target)
                .copied()
                .unwrap_or(0)
                .cast_unsigned()
        });
        row.path = if logical.is_empty() {
            suffix
        } else {
            format!("{logical}/{suffix}")
        };
        let mut entry = alias_entry(row)?;
        entry.resolved_files = resolved_files;
        entries.push(entry);
    }
    Ok(entries)
}

fn load_root_files(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<Vec<(String, u64)>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let mut rows = files::table
        .filter(files::site_id.eq(site_id))
        .select((files::path, files::size))
        .order(files::path)
        .load::<(String, i64)>(db)?;
    rows.extend(
        allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((allocated_entries::path, allocated_entries::size))
            .load::<(String, i64)>(db)?,
    );
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(rows
        .into_iter()
        .map(|(path, size)| (path, size.cast_unsigned()))
        .collect())
}

fn load_descendant_files(
    db: &mut SqliteConnection,
    name: &str,
    rel: &str,
) -> Result<Vec<(String, u64)>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let (prefix_start, prefix_end) = descendant_bounds(rel);
    let mut rows = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ge(&prefix_start))
        .filter(files::path.lt(&prefix_end))
        .select((files::path, files::size))
        .order(files::path)
        .load::<(String, i64)>(db)?;
    rows.extend(
        allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::path.ge(&prefix_start))
            .filter(allocated_entries::path.lt(&prefix_end))
            .select((allocated_entries::path, allocated_entries::size))
            .load::<(String, i64)>(db)?,
    );
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(rows
        .into_iter()
        .map(|(path, size)| (path, size.cast_unsigned()))
        .collect())
}

fn descendant_bounds(rel: &str) -> (String, String) {
    (format!("{rel}/"), format!("{rel}0"))
}

fn u64_to_f64(value: u64) -> f64 {
    const U32_RADIX: f64 = u32::MAX as f64 + 1.0;

    let high = u32::try_from(value >> u32::BITS).expect("upper half fits in u32");
    let low = u32::try_from(value & u64::from(u32::MAX)).expect("lower half fits in u32");
    f64::from(high).mul_add(U32_RADIX, f64::from(low))
}

fn quantile(sorted: &[u64], numerator: u8, denominator: u8) -> f64 {
    let position = u128::try_from(sorted.len() - 1).expect("slice length fits in u128")
        * u128::from(numerator);
    let denominator_u128 = u128::from(denominator);
    let lower = usize::try_from(position / denominator_u128).expect("quantile index fits in usize");
    let remainder =
        u8::try_from(position % denominator_u128).expect("quantile remainder fits in u8");
    let upper = lower + usize::from(remainder != 0);
    let weight = f64::from(remainder) / f64::from(denominator);
    u64_to_f64(sorted[upper]).mul_add(weight, u64_to_f64(sorted[lower]) * (1.0 - weight))
}

fn distribution(sorted: &[u64]) -> SizeDistribution {
    if sorted.is_empty() {
        return SizeDistribution {
            min: None,
            p25: None,
            median: None,
            mean: None,
            p75: None,
            max: None,
            iqr: None,
            stddev: None,
        };
    }
    let p25 = quantile(sorted, 1, 4);
    let median = quantile(sorted, 1, 2);
    let p75 = quantile(sorted, 3, 4);
    let len = u64::try_from(sorted.len()).expect("sample count fits in u64");
    let mean = sorted.iter().copied().map(u64_to_f64).sum::<f64>() / u64_to_f64(len);
    let variance = sorted
        .iter()
        .map(|size| {
            let delta = u64_to_f64(*size) - mean;
            delta * delta
        })
        .sum::<f64>()
        / u64_to_f64(len);
    SizeDistribution {
        min: sorted.first().copied(),
        p25: Some(p25),
        median: Some(median),
        mean: Some(mean),
        p75: Some(p75),
        max: sorted.last().copied(),
        iqr: Some(p75 - p25),
        stddev: Some(variance.sqrt()),
    }
}

fn dirents(files: &[(String, u64)], rel: &str) -> DirList {
    let prefix = if rel.is_empty() {
        String::new()
    } else {
        format!("{rel}/")
    };
    let mut dirs: Vec<DirEnt> = Vec::new();
    let mut direct: Vec<DirEnt> = Vec::new();
    let mut total_files = 0;
    let mut total_bytes = 0;
    for (path, size) in files {
        if is_noise_path(Path::new(path)) {
            continue;
        }
        let rest = if prefix.is_empty() {
            path.as_str()
        } else if let Some(r) = path.strip_prefix(&prefix) {
            r
        } else {
            continue;
        };
        total_files += 1;
        total_bytes += size;
        if let Some((dir, _)) = rest.split_once('/') {
            if dirs.last().is_none_or(|entry| entry.name != dir) {
                dirs.push(DirEnt {
                    kind: EntryKind::Directory,
                    name: dir.to_string(),
                    files: 0,
                    bytes: 0,
                });
            }
            let entry = dirs.last_mut().unwrap();
            entry.files += 1;
            entry.bytes += size;
        } else {
            direct.push(DirEnt {
                kind: EntryKind::File,
                name: rest.to_string(),
                files: 1,
                bytes: *size,
            });
        }
    }
    dirs.extend(direct);
    DirList {
        files: total_files,
        bytes: total_bytes,
        alias_count: 0,
        aliases: Vec::new(),
        entries: dirs,
    }
}

fn stage_dir(dir: &Path) -> io::Result<Vec<StagedFile>> {
    let mut files = Vec::new();
    collect_stage(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_stage(base: &Path, dir: &Path, out: &mut Vec<StagedFile>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_stage(base, &entry.path(), out)?;
        } else if ft.is_file() {
            let rel = entry
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(file) = stage_file(&rel, entry.path())? {
                out.push(file);
            }
        }
    }
    Ok(())
}

fn sanitized_file_properties(source: &Path) -> Result<(i64, String, TokenCounts), StoreError> {
    let sanitized = sanitize::sanitize_file(source)?;
    let mut file = fs::File::open(source)?;
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read).expect("read size fits in u64"))
            .ok_or(StoreError::SpliceResultTooLarge)?;
    }
    Ok((
        i64::try_from(size).map_err(|_| StoreError::SpliceResultTooLarge)?,
        hasher.finalize().to_hex().to_string(),
        sanitized,
    ))
}

fn stage_temporary_file(path: &str, source: PathBuf) -> Result<StagedFile, StoreError> {
    let (size, hash, sanitized) = sanitized_file_properties(&source)?;
    Ok(StagedFile {
        path: path.to_string(),
        size,
        hash,
        source: StagedSource::Temporary(source),
        sanitized,
    })
}

fn stage_borrowed_file(path: &str, source: PathBuf) -> Result<StagedFile, StoreError> {
    let (size, hash, sanitized) = sanitized_file_properties(&source)?;
    Ok(StagedFile {
        path: path.to_string(),
        size,
        hash,
        source: StagedSource::File(source),
        sanitized,
    })
}

fn stage_bytes(path: &str, bytes: &[u8]) -> StagedFile {
    let sanitized = sanitize::redact_tokens(bytes);
    let bytes = sanitized.as_bytes();
    let hash = blake3::hash(bytes);
    StagedFile {
        path: path.to_string(),
        size: i64::try_from(bytes.len()).expect("file size fits in SQLite INTEGER"),
        hash: hash.to_hex().to_string(),
        source: StagedSource::Bytes(bytes.to_vec()),
        sanitized: sanitized.counts(),
    }
}

fn stage_file(path: &str, source: PathBuf) -> io::Result<Option<StagedFile>> {
    let sanitized = sanitize::sanitize_file(&source)?;
    let mut file = fs::File::open(&source)?;
    let mut hasher = blake3::Hasher::new();
    let mut prefix = [0_u8; 4];
    let mut prefix_len = 0;
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if prefix_len < prefix.len() {
            let copied = (prefix.len() - prefix_len).min(read);
            prefix[prefix_len..prefix_len + copied].copy_from_slice(&buffer[..copied]);
            prefix_len += copied;
        }
        hasher.update(&buffer[..read]);
        size += u64::try_from(read).expect("read size fits in u64");
    }
    if is_junk(Path::new(path), Some(&prefix[..prefix_len])) {
        return Ok(None);
    }
    Ok(Some(StagedFile {
        path: path.to_string(),
        size: i64::try_from(size).expect("file size fits in SQLite INTEGER"),
        hash: hasher.finalize().to_hex().to_string(),
        source: StagedSource::File(source),
        sanitized,
    }))
}

#[cfg(test)]
fn site_files(
    db: &mut SqliteConnection,
    blobs: &BlobFiles,
    name: &str,
) -> Result<SiteArchive, StoreError> {
    let entries = site_manifest(db, name)?;
    let mut files = Vec::new();
    for entry in entries {
        match entry {
            ArchiveEntry::File { path, hash, .. } => {
                let bytes = blobs.read(&hash)?;
                if !is_junk(Path::new(&path), Some(&bytes)) {
                    files.push(ArchiveFile::File { path, bytes });
                }
            }
            ArchiveEntry::Alias { path, target } => {
                files.push(ArchiveFile::Alias { path, target });
            }
        }
    }
    Ok(SiteArchive { files })
}

fn site_manifest(db: &mut SqliteConnection, name: &str) -> Result<Vec<ArchiveEntry>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .select((files::path, files::hash, files::size))
        .order(files::path)
        .load::<(String, String, i64)>(db)?
        .into_iter()
        .map(|(path, hash, size)| ArchiveEntry::File {
            path,
            hash,
            size: size.cast_unsigned(),
        })
        .collect::<Vec<_>>();
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((
            allocated_entries::path,
            allocated_entries::hash,
            allocated_entries::size,
        ))
        .load::<(String, String, i64)>(db)?;
    for (path, hash, size) in allocated {
        entries.push(ArchiveEntry::File {
            hash,
            path,
            size: size.cast_unsigned(),
        });
    }
    entries.extend(
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?
            .into_iter()
            .map(|(path, target)| ArchiveEntry::Alias { path, target }),
    );
    entries.sort_unstable_by(|left, right| archive_path(left).cmp(archive_path(right)));
    Ok(entries)
}

fn archive_path(entry: &ArchiveEntry) -> &str {
    match entry {
        ArchiveEntry::File { path, .. } | ArchiveEntry::Alias { path, .. } => path,
    }
}

fn write_site_archive(
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
    format: ArchiveFormat,
    output: &Path,
) -> io::Result<()> {
    match format {
        ArchiveFormat::Tar => {
            append_tar_entries(fs::File::create(output)?, blobs, files)?;
        }
        ArchiveFormat::TarGz => {
            let encoder = GzEncoder::new(fs::File::create(output)?, Compression::default());
            append_tar_entries(encoder, blobs, files)?.finish()?;
        }
        ArchiveFormat::Zip => write_zip_entries(fs::File::create(output)?, blobs, files)?,
    }
    Ok(())
}

fn append_tar_entries<W: Write>(
    writer: W,
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
) -> io::Result<W> {
    let mut archive = tar::Builder::new(writer);
    for entry in files {
        match entry {
            ArchiveEntry::File { path, hash, size } => {
                let mut header = tar::Header::new_gnu();
                header.set_size(*size);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append_data(&mut header, path, fs::File::open(blobs.path(hash))?)?;
            }
            ArchiveEntry::Alias { path, target } => {
                let relative = relative_alias_target(path, target);
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                archive.append_link(&mut header, path, relative)?;
            }
        }
    }
    archive.into_inner()
}

fn write_zip_entries(
    writer: fs::File,
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
) -> io::Result<()> {
    let mut archive = zip::ZipWriter::new(writer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for entry in files {
        match entry {
            ArchiveEntry::File { path, hash, .. } => {
                archive
                    .start_file(path, options)
                    .map_err(io::Error::other)?;
                io::copy(&mut fs::File::open(blobs.path(hash))?, &mut archive)?;
            }
            ArchiveEntry::Alias { path, target } => {
                let relative = zip_safe_relative_alias_target(path, target)?;
                archive
                    .add_symlink(
                        path,
                        relative,
                        zip::write::SimpleFileOptions::default().unix_permissions(0o777),
                    )
                    .map_err(io::Error::other)?;
            }
        }
    }
    archive.finish().map(|_| ()).map_err(io::Error::other)
}

fn relative_alias_target(path: &str, target: &str) -> String {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let from = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').collect::<Vec<_>>()
    };
    let to = target.split('/').collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn zip_safe_relative_alias_target(path: &str, target: &str) -> io::Result<String> {
    let relative = relative_alias_target(path, target);
    if relative.len() > MAX_ALIAS_TARGET_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "alias target exceeds ZIP-safe limit",
        ))
    } else {
        Ok(relative)
    }
}

#[cfg(test)]
fn append_tar<W: Write>(writer: W, files: &[ArchiveFile]) -> io::Result<W> {
    let mut archive = tar::Builder::new(writer);
    for file in files {
        match file {
            ArchiveFile::File { path, bytes } => {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append_data(&mut header, path, bytes.as_slice())?;
            }
            ArchiveFile::Alias { path, target } => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                header.set_link_name(relative_alias_target(path, target))?;
                header.set_cksum();
                archive.append_data(&mut header, path, io::empty())?;
            }
        }
    }
    archive.into_inner()
}

#[cfg(test)]
fn pack_tar(files: &[ArchiveFile]) -> io::Result<Vec<u8>> {
    append_tar(Vec::new(), files)
}

#[cfg(test)]
fn pack_tar_gz(files: &[ArchiveFile]) -> io::Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    append_tar(encoder, files)?.finish()
}

#[cfg(test)]
fn pack_zip(files: &[ArchiveFile]) -> io::Result<Vec<u8>> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for file in files {
        match file {
            ArchiveFile::File { path, bytes } => {
                archive
                    .start_file(path, options)
                    .map_err(io::Error::other)?;
                archive.write_all(bytes)?;
            }
            ArchiveFile::Alias { path, target } => {
                let relative = zip_safe_relative_alias_target(path, target)?;
                archive
                    .add_symlink(
                        path,
                        relative,
                        zip::write::SimpleFileOptions::default().unix_permissions(0o777),
                    )
                    .map_err(io::Error::other)?;
            }
        }
    }
    archive
        .finish()
        .map(Cursor::into_inner)
        .map_err(io::Error::other)
}

fn gc_blobs(tx: &mut SqliteConnection, now: i64) -> Result<Vec<String>, diesel::result::Error> {
    let live_files = files::table
        .select(files::hash)
        .distinct()
        .load::<String>(tx)?;
    let live_undo = undo_files::table
        .inner_join(undo_operations::table.on(undo_operations::token.eq(undo_files::token)))
        .filter(undo_operations::consumed.eq(0_i64))
        .filter(undo_operations::expires.gt(now))
        .select(undo_files::hash)
        .distinct()
        .load::<String>(tx)?;
    let live_delta = undo_file_deltas::table
        .inner_join(undo_operations::table.on(undo_operations::token.eq(undo_file_deltas::token)))
        .filter(undo_operations::consumed.eq(0_i64))
        .filter(undo_operations::expires.gt(now))
        .filter(undo_file_deltas::hash.is_not_null())
        .select(undo_file_deltas::hash)
        .load::<Option<String>>(tx)?
        .into_iter()
        .flatten();
    let live_allocated = allocated_entries::table
        .select(allocated_entries::hash)
        .load::<String>(tx)?;
    let live_undo_allocated = undo_allocated_deltas::table
        .inner_join(
            undo_operations::table.on(undo_operations::token.eq(undo_allocated_deltas::token)),
        )
        .filter(undo_operations::consumed.eq(0_i64))
        .filter(undo_operations::expires.gt(now))
        .filter(undo_allocated_deltas::existed.eq(1_i64))
        .select(undo_allocated_deltas::hash)
        .load::<Option<String>>(tx)?
        .into_iter()
        .flatten();
    let live_pending = pending_allocations::table
        .select(pending_allocations::hash)
        .load::<String>(tx)?;
    let mut live = live_files.into_iter().collect::<HashSet<_>>();
    live.extend(live_undo);
    live.extend(live_delta);
    live.extend(live_allocated);
    live.extend(live_undo_allocated);
    live.extend(live_pending);
    let hashes = blobs::table
        .select(blobs::hash)
        .load::<String>(tx)?
        .into_iter()
        .filter(|hash| !live.contains(hash))
        .collect::<Vec<_>>();
    for chunk in hashes.chunks(SQLITE_DELETE_BATCH_SIZE) {
        diesel::delete(blobs::table.filter(blobs::hash.eq_any(chunk))).execute(tx)?;
    }
    Ok(hashes)
}

fn normalize_rel(rel: &str) -> Result<String, StoreError> {
    if rel.is_empty() {
        return Ok(String::new());
    }
    Ok(safe_rel_path(rel)?.to_string_lossy().replace('\\', "/"))
}

fn system_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).expect("timestamp fits in i64")
        })
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn map_sql(err: diesel::result::Error) -> StoreError {
    match err {
        diesel::result::Error::NotFound => StoreError::NotFound,
        other => StoreError::Sqlite(other),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn test_connection(path: &Path) -> SqliteConnection {
        SqliteConnection::establish(&path.to_string_lossy()).unwrap()
    }

    fn schema_version(db: &mut SqliteConnection) -> i64 {
        database::migrations::schema_version(db).unwrap()
    }

    struct TestClock {
        millis: AtomicU64,
    }

    #[derive(Default)]
    struct ReferenceAliases {
        files: HashSet<String>,
        aliases: BTreeMap<String, String>,
    }

    impl ReferenceAliases {
        fn resolves_file(&self, path: &str) -> bool {
            let mut current = path.to_string();
            let mut visited = HashSet::new();
            for _ in 0..MAX_ALIAS_HOPS {
                if self.files.contains(&current) {
                    return true;
                }
                if !visited.insert(current.clone()) {
                    return false;
                }
                let Some(target) = self.aliases.get(&current) else {
                    return false;
                };
                current.clone_from(target);
            }
            false
        }
    }

    impl TestClock {
        fn new(millis: u64) -> Self {
            Self {
                millis: AtomicU64::new(millis),
            }
        }

        fn advance(&self, millis: u64) {
            self.millis.fetch_add(millis, Ordering::Relaxed);
        }
    }

    impl Clock for TestClock {
        fn now_millis(&self) -> i64 {
            i64::try_from(self.millis.load(Ordering::Relaxed)).expect("test time fits in i64")
        }
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn aliases_resolve_files_chains_directories_and_dangling_targets() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("alias-test", "assets/app.js", b"app")
            .unwrap();

        let first = store
            .put_alias(
                "alias-test",
                "latest.js",
                "assets/app.js",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(first.created);
        let direct = store.alias("alias-test", "latest.js").unwrap();
        assert_eq!(direct.resolved_kind, Some(AliasResolvedKind::File));
        assert_eq!(direct.resolved_size, Some(3));
        let Node::File { hash, .. } = store.lookup("alias-test", "latest.js").unwrap() else {
            panic!("file alias must resolve");
        };
        assert_eq!(Some(hash), direct.resolved_hash);

        store
            .put_aliases(
                "alias-test",
                &[
                    AliasSpec {
                        path: "chain.js",
                        target: "latest.js",
                    },
                    AliasSpec {
                        path: "static",
                        target: "assets",
                    },
                    AliasSpec {
                        path: "missing",
                        target: "future/file.txt",
                    },
                ],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(matches!(
            store.lookup("alias-test", "chain.js"),
            Ok(Node::File { .. })
        ));
        assert!(matches!(
            store.lookup("alias-test", "static/app.js"),
            Ok(Node::File { .. })
        ));
        let static_listing = store.list_dir("alias-test", "static").unwrap();
        assert_eq!(static_listing.files, 1);
        assert_eq!(static_listing.entries[0].name, "app.js");
        assert!(matches!(
            store.lookup("alias-test", "missing"),
            Err(StoreError::NotFound)
        ));
        store
            .put_file("alias-test", "future/file.txt", b"future")
            .unwrap();
        assert!(matches!(
            store.lookup("alias-test", "missing"),
            Ok(Node::File { .. })
        ));
        let deleted = store.delete_file("alias-test", "future/file.txt").unwrap();
        assert_eq!(
            store.alias("alias-test", "missing").unwrap().resolved_kind,
            None
        );
        store
            .undo(
                "alias-test",
                deleted.undo.as_ref().map(|undo| undo.token.as_str()),
            )
            .unwrap();
        assert!(matches!(
            store.lookup("alias-test", "missing"),
            Ok(Node::File { .. })
        ));
        assert_eq!(store.aliases("alias-test").unwrap().len(), 4);
        let inventory = store.alias_inventory("alias-test").unwrap();
        assert_eq!(inventory.aliases.len(), 4);
        assert_eq!(
            store.alias_stats("alias-test").unwrap(),
            AliasStats {
                aliases: 4,
                resolved: 4,
                dangling: 0,
            }
        );
    }

    #[test]
    fn directory_listings_include_nested_aliases_without_inflating_files_or_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("alias-list", "tree/file.txt", b"1234")
            .unwrap();
        let baseline = store.list_dir("alias-list", "").unwrap();
        store
            .put_alias(
                "alias-list",
                "tree/nested/link",
                "../file.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .put_alias("alias-list", "view", "tree", FileMutationOptions::default())
            .unwrap();

        let root = store.list_dir("alias-list", "").unwrap();
        assert_eq!(root.alias_count, 2);
        assert_eq!(root.files, baseline.files);
        assert!(
            root.entries
                .iter()
                .all(|entry| entry.name != "view" && entry.name != "link")
        );
        let tree = store.list_dir("alias-list", "tree").unwrap();
        assert_eq!(tree.alias_count, 1);
        assert_eq!(tree.aliases[0].path, "tree/nested/link");
        assert_eq!(tree.files, 1);
        let view = store.list_dir("alias-list", "view").unwrap();
        assert_eq!(view.alias_count, 1);
        assert_eq!(view.aliases[0].path, "view/nested/link");
        assert_eq!(view.files, 1);
        assert_eq!(view.bytes, 4);
    }

    #[test]
    fn alias_security_conflicts_cycles_no_follow_and_undo_are_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-safe", "real/file.txt", b"x").unwrap();
        assert!(matches!(
            store.put_alias(
                "alias-safe",
                "bad",
                "../../outside",
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAliasTarget)
        ));
        assert!(matches!(
            store.put_alias(
                "alias-safe",
                "real/file.txt",
                "other",
                FileMutationOptions::default()
            ),
            Err(StoreError::AliasConflict)
        ));
        store
            .put_alias("alias-safe", "a", "b", FileMutationOptions::default())
            .unwrap();
        assert!(matches!(
            store.put_alias("alias-safe", "b", "a", FileMutationOptions::default()),
            Err(StoreError::AliasCycle)
        ));
        assert!(store.alias("alias-safe", "b").is_err());
        store
            .put_alias(
                "alias-safe",
                "linked",
                "real",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(matches!(
            store.put_file("alias-safe", "linked/new.txt", b"no"),
            Err(StoreError::AliasWrite)
        ));
        let mutation = store
            .put_alias(
                "alias-safe",
                "temporary",
                "real/file.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .undo(
                "alias-safe",
                mutation.undo.as_ref().map(|undo| undo.token.as_str()),
            )
            .unwrap();
        assert!(store.alias("alias-safe", "temporary").is_err());

        let chain = (0..65)
            .map(|index| {
                (
                    format!("hop-{index:02}"),
                    if index == 64 {
                        "real/file.txt".to_string()
                    } else {
                        format!("hop-{:02}", index + 1)
                    },
                )
            })
            .collect::<Vec<_>>();
        let specs = chain
            .iter()
            .map(|(path, target)| AliasSpec { path, target })
            .collect::<Vec<_>>();
        assert!(matches!(
            store.put_aliases("alias-safe", &specs, FileMutationOptions::default()),
            Err(StoreError::AliasHopLimit)
        ));
        assert!(store.alias("alias-safe", "hop-00").is_err());
    }

    #[test]
    fn aliases_reject_containment_cycles_and_prefix_shadowing_in_both_orders() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-prefix", "seed.txt", b"x").unwrap();
        assert!(matches!(
            store.put_alias(
                "alias-prefix",
                "dir/link",
                ".",
                FileMutationOptions::default()
            ),
            Err(StoreError::AliasCycle)
        ));
        assert!(matches!(
            store.put_aliases(
                "alias-prefix",
                &[
                    AliasSpec {
                        path: "root-link",
                        target: "dir",
                    },
                    AliasSpec {
                        path: "dir/back",
                        target: "../root-link",
                    },
                ],
                FileMutationOptions::default(),
            ),
            Err(StoreError::AliasCycle)
        ));

        store
            .put_file("alias-prefix", "existing/child.txt", b"x")
            .unwrap();
        assert!(matches!(
            store.put_alias(
                "alias-prefix",
                "existing",
                "seed.txt",
                FileMutationOptions::default()
            ),
            Err(StoreError::AliasConflict)
        ));

        store
            .put_alias(
                "alias-prefix",
                "first",
                "seed.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(matches!(
            store.put_file("alias-prefix", "first/child.txt", b"x"),
            Err(StoreError::AliasWrite)
        ));
        assert!(matches!(
            store.put_alias(
                "alias-prefix",
                "first/child",
                "seed.txt",
                FileMutationOptions::default()
            ),
            Err(StoreError::AliasWrite)
        ));
    }

    #[test]
    fn alias_targets_allow_safe_punctuation_but_reject_noise_controls_and_external_uris() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        for target in ["name:part", "query?part", "hash#part"] {
            store
                .put_file("alias-paths", target, target.as_bytes())
                .unwrap();
            store
                .put_alias(
                    "alias-paths",
                    &format!("link-{}", target.len()),
                    target,
                    FileMutationOptions::default(),
                )
                .unwrap();
        }
        for target in [
            "https://example.test/file",
            "mailto:user@example.test",
            "data:text/plain,x",
            "file:///tmp/x",
            ".DS_Store",
            "dir/\nname",
        ] {
            assert!(matches!(
                store.put_alias(
                    "alias-paths",
                    "rejected",
                    target,
                    FileMutationOptions::default()
                ),
                Err(StoreError::InvalidAliasTarget)
            ));
        }
        for path in ["bad\npath", "/absolute", r"back\slash", ".DS_Store"] {
            assert!(matches!(
                store.put_alias(
                    "alias-paths",
                    path,
                    "name:part",
                    FileMutationOptions::default()
                ),
                Err(StoreError::InvalidAliasTarget)
            ));
        }
    }

    #[test]
    fn alias_retarget_reports_created_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-created", "one", b"1").unwrap();
        store.put_file("alias-created", "two", b"2").unwrap();
        store
            .put_alias(
                "alias-created",
                "link",
                "one",
                FileMutationOptions::default(),
            )
            .unwrap();
        let retargeted = store
            .put_alias(
                "alias-created",
                "link",
                "two",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(!retargeted.created);
        assert!(retargeted.changed);
    }

    #[test]
    fn alias_batch_database_failure_rolls_back_every_row_and_undo_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-failure", "target", b"x").unwrap();
        let undo_count = store.undo_stack("alias-failure").unwrap().entries.len();
        {
            let mut db = store.inner.writer.lock().unwrap();
            db.batch_execute(
                "CREATE TEMP TRIGGER fail_second_alias
                 BEFORE INSERT ON aliases
                 WHEN NEW.path = 'b'
                 BEGIN
                   SELECT RAISE(ABORT, 'injected alias failure');
                 END;",
            )
            .unwrap();
        }
        assert!(
            store
                .put_aliases(
                    "alias-failure",
                    &[
                        AliasSpec {
                            path: "a",
                            target: "target",
                        },
                        AliasSpec {
                            path: "b",
                            target: "target",
                        },
                    ],
                    FileMutationOptions::default(),
                )
                .is_err()
        );
        assert!(store.aliases("alias-failure").unwrap().is_empty());
        assert_eq!(
            store.undo_stack("alias-failure").unwrap().entries.len(),
            undo_count
        );
    }

    #[test]
    fn alias_inventory_identity_and_rows_share_one_reader_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-snapshot", "target", b"x").unwrap();
        store
            .put_alias(
                "alias-snapshot",
                "first",
                "target",
                FileMutationOptions::default(),
            )
            .unwrap();
        let initial = store.alias_inventory("alias-snapshot").unwrap();
        let (identity_tx, identity_rx) = std::sync::mpsc::sync_channel(0);
        let (continue_tx, continue_rx) = std::sync::mpsc::sync_channel(0);
        {
            let mut readers = store.inner.readers.available.lock().unwrap();
            let db = readers.last_mut().unwrap();
            let mut paused = false;
            db.set_instrumentation(move |event: diesel::connection::InstrumentationEvent<'_>| {
                if paused {
                    return;
                }
                if let diesel::connection::InstrumentationEvent::FinishQuery {
                    query,
                    error: None,
                    ..
                } = event
                {
                    let sql = query.to_string();
                    if sql.contains("content_revision") && sql.contains("sites") {
                        paused = true;
                        identity_tx.send(()).unwrap();
                        continue_rx.recv().unwrap();
                    }
                }
            });
            drop(readers);
        }
        let reader_store = store.clone();
        let reader =
            std::thread::spawn(move || reader_store.alias_inventory("alias-snapshot").unwrap());
        identity_rx.recv().unwrap();
        store
            .put_alias(
                "alias-snapshot",
                "second",
                "target",
                FileMutationOptions::default(),
            )
            .unwrap();
        continue_tx.send(()).unwrap();
        let snapshot = reader.join().unwrap();
        assert_eq!(snapshot.content_revision, initial.content_revision);
        assert_eq!(snapshot.aliases, initial.aliases);
        assert_eq!(store.aliases("alias-snapshot").unwrap().len(), 2);
    }

    #[test]
    fn alias_archives_export_relative_symlink_metadata_for_tar_and_zip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store")).unwrap();
        store
            .put_file("alias-archive", "assets/app.js", b"app")
            .unwrap();
        store
            .put_alias(
                "alias-archive",
                "current/app.js",
                "../assets/app.js",
                FileMutationOptions::default(),
            )
            .unwrap();
        for (format, kind, extension) in [
            (ArchiveFormat::Tar, Kind::Tar, "tar"),
            (ArchiveFormat::Zip, Kind::Zip, "zip"),
        ] {
            let archive = dir.path().join(format!("site.{extension}"));
            store
                .pack_site_to_path("alias-archive", format, &archive)
                .unwrap();
            let plan = crate::upload::plan_archive(&archive, kind).unwrap();
            assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
                path: "current/app.js".to_string(),
                canonical_target: "assets/app.js".to_string(),
            }));
        }
    }

    #[test]
    fn tar_alias_export_supports_gnu_long_link_targets() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store")).unwrap();
        let target = format!("{}/file.txt", "long".repeat(30));
        store.put_file("alias-long", &target, b"x").unwrap();
        store
            .put_alias(
                "alias-long",
                "link",
                &target,
                FileMutationOptions::default(),
            )
            .unwrap();
        let archive = dir.path().join("long.tar");
        store
            .pack_site_to_path("alias-long", ArchiveFormat::Tar, &archive)
            .unwrap();
        let plan = crate::upload::plan_archive(&archive, Kind::Tar).unwrap();
        assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
            path: "link".to_string(),
            canonical_target: target,
        }));
    }

    #[test]
    fn copy_move_and_whole_site_undo_preserve_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-source", "file.txt", b"x").unwrap();
        store
            .put_alias(
                "alias-source",
                "link.txt",
                "file.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .copy_site("alias-source", Some("alias-copy"), None)
            .unwrap();
        assert!(matches!(
            store.lookup("alias-copy", "link.txt"),
            Ok(Node::File { .. })
        ));
        store.move_site("alias-copy", "alias-moved").unwrap();
        assert!(matches!(
            store.lookup("alias-moved", "link.txt"),
            Ok(Node::File { .. })
        ));
        store.undo("alias-moved", None).unwrap();
        assert!(matches!(
            store.lookup("alias-copy", "link.txt"),
            Ok(Node::File { .. })
        ));
        store.pop_site("alias-copy").unwrap();
        store.undo("alias-copy", None).unwrap();
        assert!(matches!(
            store.lookup("alias-copy", "link.txt"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn alias_expiry_removes_only_the_alias_and_undo_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store_clock = Arc::<TestClock>::clone(&clock);
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            store_clock,
        )
        .unwrap();
        store
            .put_file("alias-expiry", "dir/file.txt", b"x")
            .unwrap();
        store
            .put_alias(
                "alias-expiry",
                "linked",
                "dir",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "alias-expiry",
                "linked",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 1,
                }),
            )
            .unwrap();
        clock.advance(1_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(store.alias("alias-expiry", "linked").is_err());
        assert!(matches!(
            store.lookup("alias-expiry", "dir/file.txt"),
            Ok(Node::File { .. })
        ));
        store.undo("alias-expiry", None).unwrap();
        assert!(matches!(
            store.lookup("alias-expiry", "linked/file.txt"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn alias_expiry_is_zero_cost_dangling_safe_and_rejects_paths_below_directory_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store
            .put_file("alias-expiry-rules", "target/file.txt", b"1234")
            .unwrap();
        let baseline = store.list_sites().unwrap();
        store
            .put_alias(
                "alias-expiry-rules",
                "linked",
                "target",
                FileMutationOptions::default(),
            )
            .unwrap();
        let after_alias = store.list_sites().unwrap();
        assert_eq!(after_alias.files, baseline.files);
        assert_eq!(after_alias.bytes, baseline.bytes);
        assert!(matches!(
            store.set_expiry(
                "alias-expiry-rules",
                "linked/file.txt",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 60
                })
            ),
            Err(StoreError::AliasWrite)
        ));
        store
            .set_expiry(
                "alias-expiry-rules",
                "linked",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 60,
                }),
            )
            .unwrap();
        store
            .delete_file("alias-expiry-rules", "target/file.txt")
            .unwrap();
        assert_eq!(
            store
                .alias("alias-expiry-rules", "linked")
                .unwrap()
                .resolved_kind,
            None
        );
        assert!(store.expiry_report("alias-expiry-rules", "linked").is_ok());
        clock.advance(60_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(store.alias("alias-expiry-rules", "linked").is_err());
    }

    #[test]
    fn alias_retarget_and_dependency_changes_refresh_expiry_kind_size_and_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store_clock = Arc::<TestClock>::clone(&clock);
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            store_clock,
        )
        .unwrap();
        store.put_file("alias-policy", "small", b"x").unwrap();
        store
            .put_file("alias-policy", "folder/large", &[0_u8; 4096])
            .unwrap();
        store
            .put_alias(
                "alias-policy",
                "linked",
                "small",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "alias-policy",
                "linked",
                Some(ExpiryPolicy::Decay(DecayPolicy {
                    min_age_seconds: 10,
                    max_age_seconds: 1_000,
                    max_size_bytes: 10_000,
                    power: 1.0,
                })),
            )
            .unwrap();
        let site_id = {
            let mut db = store.inner.readers.get();
            site_id_locked(&mut db, "alias-policy").unwrap()
        };
        let before = {
            let mut db = store.inner.readers.get();
            load_expiry_policy_locked(&mut db, site_id, "linked")
                .unwrap()
                .unwrap()
        };
        clock.advance(1_000);
        store
            .put_alias(
                "alias-policy",
                "linked",
                "folder",
                FileMutationOptions::default(),
            )
            .unwrap();
        let after = {
            let mut db = store.inner.readers.get();
            load_expiry_policy_locked(&mut db, site_id, "linked")
                .unwrap()
                .unwrap()
        };
        assert_eq!(after.kind, ExpiryTargetKind::Folder);
        assert_eq!(after.size_bytes, 0);
        assert_ne!(after.own_deadline_millis, before.own_deadline_millis);

        clock.advance(1_000);
        store
            .put_file("alias-policy", "folder/another", &[0_u8; 4096])
            .unwrap();
        let dependency_changed = {
            let mut db = store.inner.readers.get();
            load_expiry_policy_locked(&mut db, site_id, "linked")
                .unwrap()
                .unwrap()
        };
        assert_eq!(dependency_changed.size_bytes, 0);
        assert_ne!(
            dependency_changed.own_deadline_millis,
            after.own_deadline_millis
        );
    }

    #[test]
    fn intermediate_alias_retarget_refreshes_transitive_alias_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store.put_file("alias-transitive", "one", b"1").unwrap();
        store.put_file("alias-transitive", "two", b"22").unwrap();
        store
            .put_alias(
                "alias-transitive",
                "middle",
                "one",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .put_alias(
                "alias-transitive",
                "outer",
                "middle",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "alias-transitive",
                "outer",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 60,
                }),
            )
            .unwrap();
        let before = store.expiry_report("alias-transitive", "outer").unwrap();
        clock.advance(1_000);
        store
            .put_alias(
                "alias-transitive",
                "middle",
                "two",
                FileMutationOptions::default(),
            )
            .unwrap();
        let after = store.expiry_report("alias-transitive", "outer").unwrap();
        assert_ne!(after.refreshed_at, before.refreshed_at);
        assert_ne!(after.effective_expires_at, before.effective_expires_at);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn alias_expiry_caps_follow_targets_chains_directories_and_dangling_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store
            .put_file("alias-caps", "target.txt", b"target")
            .unwrap();
        store
            .put_file("alias-caps", "directory/item.txt", b"item")
            .unwrap();
        store
            .put_file("alias-caps", "links/anchor.txt", b"anchor")
            .unwrap();
        let target = store
            .set_expiry(
                "alias-caps",
                "target.txt",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 80,
                }),
            )
            .unwrap()
            .report;
        let directory_item = store
            .set_expiry(
                "alias-caps",
                "directory/item.txt",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 30,
                }),
            )
            .unwrap()
            .report;
        store
            .put_aliases(
                "alias-caps",
                &[
                    AliasSpec {
                        path: "links/target-capped",
                        target: "../target.txt",
                    },
                    AliasSpec {
                        path: "links/direct",
                        target: "../target.txt",
                    },
                    AliasSpec {
                        path: "links/chain",
                        target: "direct",
                    },
                    AliasSpec {
                        path: "view",
                        target: "directory",
                    },
                    AliasSpec {
                        path: "dangling",
                        target: "missing",
                    },
                ],
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "alias-caps",
                "links",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 120,
                }),
            )
            .unwrap();
        let direct = store
            .set_expiry(
                "alias-caps",
                "links/direct",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 40,
                }),
            )
            .unwrap()
            .report;
        store
            .set_expiry(
                "alias-caps",
                "links/chain",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 60,
                }),
            )
            .unwrap();
        let dangling = store
            .set_expiry(
                "alias-caps",
                "dangling",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 45,
                }),
            )
            .unwrap()
            .report;

        assert_eq!(
            store
                .expiry_report("alias-caps", "links/target-capped")
                .unwrap()
                .effective_expires_at,
            target.effective_expires_at
        );
        assert_eq!(
            store
                .expiry_report("alias-caps", "links/chain")
                .unwrap()
                .effective_expires_at,
            direct.effective_expires_at
        );
        assert_eq!(
            store
                .expiry_report("alias-caps", "view/item.txt")
                .unwrap()
                .effective_expires_at,
            directory_item.effective_expires_at
        );
        assert_eq!(
            store
                .expiry_report("alias-caps", "dangling")
                .unwrap()
                .effective_expires_at,
            dangling.effective_expires_at
        );
    }

    #[test]
    fn unrelated_write_refreshes_only_affected_alias_closure() {
        const ALIAS_COUNT: usize = 4096;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("alias-refresh", "target.txt", b"target")
            .unwrap();
        let paths = (0..ALIAS_COUNT)
            .map(|index| format!("links/{index:04}.txt"))
            .collect::<Vec<_>>();
        let specs = paths
            .iter()
            .map(|path| AliasSpec {
                path,
                target: "../target.txt",
            })
            .collect::<Vec<_>>();
        store
            .put_aliases("alias-refresh", &specs, FileMutationOptions::default())
            .unwrap();

        reset_alias_refresh_row_work();
        store
            .put_file("alias-refresh", "unrelated/new.txt", b"new")
            .unwrap();
        let work = alias_refresh_row_work();
        assert!(
            work.aliases <= 2,
            "unrelated write loaded {} of {ALIAS_COUNT} aliases",
            work.aliases
        );
        assert!(
            work.real <= 2,
            "unrelated write loaded {} unrelated real entries",
            work.real
        );
        assert!(matches!(
            store.lookup("alias-refresh", "links/4095.txt"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn directory_alias_listing_row_work_is_prefix_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-bounded", "tree/a", b"a").unwrap();
        store.put_file("alias-bounded", "tree/b", b"b").unwrap();
        store
            .allocate_bytes(
                "alias-bounded",
                b"allocated",
                AllocationSpec {
                    folder: "tree",
                    ..AllocationSpec::default()
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .put_file("alias-bounded", "elsewhere/a", b"a")
            .unwrap();
        store
            .put_file("alias-bounded", "elsewhere/b", b"b")
            .unwrap();
        for index in 0..256 {
            store
                .put_file("alias-bounded", &format!("unrelated/{index:03}"), b"x")
                .unwrap();
        }
        store
            .put_alias(
                "alias-bounded",
                "tree/nested",
                "a",
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .put_alias(
                "alias-bounded",
                "tree/external",
                "../elsewhere",
                FileMutationOptions::default(),
            )
            .unwrap();
        let unrelated_paths = (0..256)
            .map(|index| format!("unrelated-links/{index:03}"))
            .collect::<Vec<_>>();
        let unrelated = unrelated_paths
            .iter()
            .map(|path| AliasSpec {
                path,
                target: "../unrelated/000",
            })
            .collect::<Vec<_>>();
        store
            .put_aliases("alias-bounded", &unrelated, FileMutationOptions::default())
            .unwrap();
        store
            .put_alias(
                "alias-bounded",
                "view",
                "tree",
                FileMutationOptions::default(),
            )
            .unwrap();

        reset_alias_directory_row_work();
        let listing = store.list_dir("alias-bounded", "view").unwrap();
        let work = alias_directory_row_work();
        assert_eq!(listing.files, 3);
        assert_eq!(listing.alias_count, 2);
        assert_eq!(work.files, 2);
        assert_eq!(work.allocated, 1);
        assert_eq!(work.aliases, 2);
        assert_eq!(work.aggregates, 1);
        assert!(work.resolution <= 12);
    }

    #[test]
    fn alias_paths_reject_controls_and_noise_but_preserve_safe_punctuation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("alias-paths", "target.txt", b"target")
            .unwrap();
        for invalid in [
            "line\nbreak",
            "carriage\rreturn",
            "tab\tpath",
            "bell\u{7}path",
            ".DS_Store",
            "nested/Thumbs.db",
            "nested/._resource",
        ] {
            assert!(matches!(
                store.put_alias(
                    "alias-paths",
                    invalid,
                    "target.txt",
                    FileMutationOptions::default()
                ),
                Err(StoreError::InvalidAliasTarget)
            ));
        }

        let safe = "safe/quote\" ' []{}=+,;!@~$^&()#%?.txt";
        store
            .put_alias(
                "alias-paths",
                safe,
                "../target.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(matches!(
            store.lookup("alias-paths", safe),
            Ok(Node::File { .. })
        ));
        let Node::File { hash, .. } = store.lookup("alias-paths", MANIFEST_PATH).unwrap() else {
            panic!("manifest must be a file");
        };
        let manifest = String::from_utf8(store.read_blob(&hash).unwrap().to_vec()).unwrap();
        let parsed = toml::from_str::<toml::Value>(&manifest).unwrap();
        assert_eq!(
            parsed["aliases"][safe].as_str(),
            Some("target.txt"),
            "safe punctuation must round-trip through generated TOML"
        );
    }

    #[test]
    fn zip_safe_alias_target_limit_roundtrips_at_boundary_and_rejects_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store")).unwrap();
        let boundary = format!("{}aa", "a/".repeat((MAX_ALIAS_TARGET_BYTES - 2) / 2));
        assert_eq!(boundary.len(), MAX_ALIAS_TARGET_BYTES);
        store.put_file("alias-limit", "seed", b"x").unwrap();
        store
            .put_alias(
                "alias-limit",
                "link",
                &boundary,
                FileMutationOptions::default(),
            )
            .unwrap();
        let archive = dir.path().join("boundary.zip");
        store
            .pack_site_to_path("alias-limit", ArchiveFormat::Zip, &archive)
            .unwrap();
        let plan = crate::upload::plan_archive(&archive, Kind::Zip).unwrap();
        assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
            path: "link".to_string(),
            canonical_target: boundary.clone(),
        }));

        let overflow = format!("{boundary}x");
        assert!(matches!(
            store.put_alias(
                "alias-limit",
                "too-long",
                &overflow,
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAliasTarget)
        ));
    }

    #[test]
    fn alias_batch_scales_to_42802_entries_atomically() {
        const ALIAS_COUNT: usize = 42_802;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-scale", "target.txt", b"x").unwrap();
        let paths = (0..ALIAS_COUNT)
            .map(|index| format!("links/{index:05}.txt"))
            .collect::<Vec<_>>();
        let specs = paths
            .iter()
            .map(|path| AliasSpec {
                path,
                target: "../target.txt",
            })
            .collect::<Vec<_>>();
        let mutation = store
            .put_aliases("alias-scale", &specs, FileMutationOptions::default())
            .unwrap();
        assert_eq!(mutation.files, ALIAS_COUNT);
        assert_eq!(
            store.alias_stats("alias-scale").unwrap().resolved,
            ALIAS_COUNT as u64
        );
        assert!(matches!(
            store.lookup("alias-scale", "links/42801.txt"),
            Ok(Node::File { .. })
        ));
        let alias_queries = Arc::new(AtomicU64::new(0));
        {
            let counter = Arc::clone(&alias_queries);
            let mut db = store.inner.writer.lock().unwrap();
            db.set_instrumentation(move |event: diesel::connection::InstrumentationEvent<'_>| {
                if let diesel::connection::InstrumentationEvent::StartQuery { query, .. } = event {
                    let sql = query.to_string();
                    if sql.contains("SELECT") && sql.contains("aliases") {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        let hash = blake3::hash(b"x").to_hex().to_string();
        let staged = (0..ALIAS_COUNT)
            .map(|index| StagedFile {
                path: format!("files/{index:05}.txt"),
                size: 1,
                hash: hash.clone(),
                source: StagedSource::Bytes(vec![b'x']),
                sanitized: TokenCounts::default(),
            })
            .collect::<Vec<_>>();
        let file_mutation = store
            .merge_staged("alias-scale", &staged, UndoKind::Put)
            .unwrap();
        let no_op = store
            .put_aliases("alias-scale", &specs, FileMutationOptions::default())
            .unwrap();
        assert!(!no_op.changed);
        let bounded_alias_queries = alias_queries.load(Ordering::Relaxed);
        assert!(
            (1..=12).contains(&bounded_alias_queries),
            "alias query work must stay batch-bounded"
        );
        store
            .undo(
                "alias-scale",
                file_mutation.undo.as_ref().map(|undo| undo.token.as_str()),
            )
            .unwrap();
        store
            .undo(
                "alias-scale",
                mutation.undo.as_ref().map(|undo| undo.token.as_str()),
            )
            .unwrap();
        assert_eq!(store.alias_stats("alias-scale").unwrap().aliases, 0);
    }

    #[test]
    fn alias_cache_refresh_writes_only_rows_whose_resolution_changed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-cache", "target", b"x").unwrap();
        let paths = (0..128)
            .map(|index| format!("links/{index:03}"))
            .collect::<Vec<_>>();
        let specs = paths
            .iter()
            .map(|path| AliasSpec {
                path,
                target: "../target",
            })
            .collect::<Vec<_>>();
        store
            .put_aliases("alias-cache", &specs, FileMutationOptions::default())
            .unwrap();
        {
            let mut db = store.inner.writer.lock().unwrap();
            db.batch_execute(
                "CREATE TEMP TABLE alias_update_count (count INTEGER NOT NULL);
                 INSERT INTO alias_update_count VALUES (0);
                 CREATE TEMP TRIGGER count_alias_cache_updates
                 BEFORE UPDATE ON aliases
                 BEGIN
                   UPDATE alias_update_count SET count = count + 1;
                 END;",
            )
            .unwrap();
        }
        store.put_file("alias-cache", "unrelated", b"u").unwrap();
        {
            let mut db = store.inner.writer.lock().unwrap();
            let count = diesel::sql_query("SELECT count FROM alias_update_count")
                .get_result::<AliasUpdateCount>(&mut *db)
                .unwrap()
                .count;
            assert_eq!(count, 0);
            diesel::sql_query("UPDATE alias_update_count SET count = 0")
                .execute(&mut *db)
                .unwrap();
        }
        store.put_file("alias-cache", "target", b"changed").unwrap();
        let mut db = store.inner.writer.lock().unwrap();
        let count = diesel::sql_query("SELECT count FROM alias_update_count")
            .get_result::<AliasUpdateCount>(&mut *db)
            .unwrap()
            .count;
        drop(db);
        assert_eq!(count, 128);
    }

    #[test]
    fn deterministic_alias_sequences_match_the_simple_reference_model() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-model", "seed.txt", b"seed").unwrap();
        let mut reference = ReferenceAliases::default();
        reference.files.insert("seed.txt".to_string());
        let mut state = 0x42_u64;
        for index in 0..256 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let path = format!("alias-{index:03}");
            let target = match state % 5 {
                0 => "seed.txt".to_string(),
                1 if index > 0 => format!(
                    "alias-{:03}",
                    usize::try_from(state % u64::try_from(index).expect("index fits in u64"))
                        .expect("bounded index fits in usize")
                ),
                _ => format!("missing-{:03}", state % 31),
            };
            store
                .put_alias(
                    "alias-model",
                    &path,
                    &target,
                    FileMutationOptions::default(),
                )
                .unwrap();
            reference.aliases.insert(path.clone(), target);
            if index > 0 && index % 11 == 0 {
                let retargeted = format!("alias-{:03}", index / 2);
                store
                    .put_alias(
                        "alias-model",
                        &retargeted,
                        "seed.txt",
                        FileMutationOptions::default(),
                    )
                    .unwrap();
                reference.aliases.insert(retargeted, "seed.txt".to_string());
            }
            if index > 0 && index % 53 == 0 {
                let deleted = format!("alias-{:03}", index / 3);
                store.delete_file("alias-model", &deleted).unwrap();
                reference.aliases.remove(&deleted);
            }
            if index > 0 && index % 37 == 0 {
                store.delete_file("alias-model", "seed.txt").unwrap();
                reference.files.remove("seed.txt");
                for alias in reference.aliases.keys() {
                    assert_eq!(
                        store.lookup("alias-model", alias).is_ok(),
                        reference.resolves_file(alias)
                    );
                }
                store.put_file("alias-model", "seed.txt", b"seed").unwrap();
                reference.files.insert("seed.txt".to_string());
            }
            for alias in reference.aliases.keys() {
                assert_eq!(
                    store.lookup("alias-model", alias).is_ok(),
                    reference.resolves_file(alias)
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn generated_alias_sequences_match_the_reference_model(
            choices in prop::collection::vec((any::<u8>(), any::<bool>()), 1..96)
        ) {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store.put_file("alias-property", "seed.txt", b"seed").unwrap();
            let mut reference = ReferenceAliases::default();
            reference.files.insert("seed.txt".to_string());

            for (index, (choice, retarget)) in choices.into_iter().enumerate() {
                let path = format!("alias-{index:03}");
                let target = match choice % 3 {
                    0 => "seed.txt".to_string(),
                    1 if index > 0 => format!("alias-{:03}", usize::from(choice) % index),
                    _ => format!("missing-{:03}", choice % 17),
                };
                store
                    .put_alias(
                        "alias-property",
                        &path,
                        &target,
                        FileMutationOptions::default(),
                    )
                    .unwrap();
                reference.aliases.insert(path.clone(), target);

                if retarget && index > 0 {
                    let changed = format!("alias-{:03}", usize::from(choice) % index);
                    store
                        .put_alias(
                            "alias-property",
                            &changed,
                            "seed.txt",
                            FileMutationOptions::default(),
                        )
                        .unwrap();
                    reference.aliases.insert(changed, "seed.txt".to_string());
                }

                for alias in reference.aliases.keys() {
                    prop_assert_eq!(
                        store.lookup("alias-property", alias).is_ok(),
                        reference.resolves_file(alias),
                        "generated step {}, alias {}",
                        index,
                        alias,
                    );
                }
            }
        }

        #[test]
        fn generated_control_characters_are_rejected_from_alias_paths(
            prefix in "[a-z]{0,12}",
            suffix in "[a-z]{0,12}",
            control in 0_u8..=31,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store.put_file("alias-fuzz", "target", b"x").unwrap();
            let invalid = format!("{prefix}{}{suffix}", char::from(control));
            prop_assert!(store
                .put_alias(
                    "alias-fuzz",
                    &invalid,
                    "target",
                    FileMutationOptions::default(),
                )
                .is_err());
        }
    }

    #[test]
    fn direct_alias_read_cost_ignores_unrelated_dangling_aliases() {
        const UNRELATED: usize = 5_000;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("alias-read-cost", "target", b"value")
            .unwrap();
        store
            .put_alias(
                "alias-read-cost",
                "direct",
                "target",
                FileMutationOptions::default(),
            )
            .unwrap();
        let paths = (0..UNRELATED)
            .map(|index| format!("noise/{index:04}"))
            .collect::<Vec<_>>();
        let targets = (0..UNRELATED)
            .map(|index| format!("../missing/{index:04}"))
            .collect::<Vec<_>>();
        let aliases = paths
            .iter()
            .zip(&targets)
            .map(|(path, target)| AliasSpec { path, target })
            .collect::<Vec<_>>();
        store
            .put_aliases("alias-read-cost", &aliases, FileMutationOptions::default())
            .unwrap();

        reset_alias_directory_row_work();
        assert!(matches!(
            store.lookup("alias-read-cost", "direct"),
            Ok(Node::File { .. })
        ));
        let work = alias_directory_row_work();
        assert!(
            work.resolution <= 1,
            "direct alias read inspected {} rows with {UNRELATED} unrelated aliases",
            work.resolution
        );
    }

    #[test]
    fn migrations_merge_manifest_and_reserved_paths_follow_contract() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_public_url(
            dir.path().to_path_buf(),
            "https://symbol.example".to_string(),
        )
        .unwrap();
        store
            .replace_site("hello", b"first", Kind::Html, None, false)
            .unwrap();
        store
            .replace_site("hello", b"second", Kind::File, Some("other.txt"), false)
            .unwrap();

        assert!(matches!(
            store.lookup("hello", "index.html"),
            Ok(Node::File { .. })
        ));
        let Node::File { hash, .. } = store.lookup("hello", MANIFEST_PATH).unwrap() else {
            panic!("manifest must be stored");
        };
        let manifest = String::from_utf8(store.read_blob(&hash).unwrap().to_vec()).unwrap();
        assert!(manifest.contains("host = \"https://symbol.example\""));
        assert!(manifest.contains("name = \"hello\""));
        assert!(manifest.contains("content_revision = 2"));
        assert!(manifest.contains("\"index.html\" = \"blake3:"));
        assert!(manifest.contains("\"other.txt\" = \"blake3:"));
        assert!(matches!(
            store.put_file("hello", "nested/UNDO", b"no"),
            Err(StoreError::Upload(UploadError::ReservedPath))
        ));
        let mut db = test_connection(&dir.path().join("symbol.db"));
        assert_eq!(
            schema_version(&mut db),
            database::schema::LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn production_v6_database_is_safely_baselined() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("symbol.db");
        let mut db = test_connection(&path);
        database::migrations::migrate(&mut db).unwrap();
        diesel::insert_into(sites::table)
            .values(NewSite {
                name: "existing",
                updated: 1,
                public_url: "https://symbol.example",
                content_revision: 7,
                tree_hash: "blake3:existing",
                creator_kind: None,
                creator_hash: None,
                claim_hash: None,
                management_hash: None,
                management_status: 0,
            })
            .execute(&mut db)
            .unwrap();
        run_migrations(&mut db).unwrap();
        run_migrations(&mut db).unwrap();
        assert_eq!(
            schema_version(&mut db),
            database::schema::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            sites::table
                .filter(sites::name.eq("existing"))
                .select(sites::content_revision)
                .first::<i64>(&mut db)
                .unwrap(),
            7
        );
    }

    #[test]
    fn migration_integrity_gate_rejects_foreign_key_violations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("symbol.db");
        let mut db = test_connection(&path);
        run_migrations(&mut db).unwrap();
        db.batch_execute("PRAGMA foreign_keys = OFF").unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                site_id: 999,
                path: "orphan.txt",
                hash: "missing",
                size: 1,
            })
            .execute(&mut db)
            .unwrap();
        db.batch_execute("PRAGMA foreign_keys = ON").unwrap();
        assert!(matches!(
            run_migrations(&mut db),
            Err(StoreError::Io(error))
                if error.to_string() == "2 foreign-key violations"
        ));
    }

    #[test]
    fn production_shaped_v2_database_migrates_through_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("symbol.db");
        let mut db = test_connection(&path);
        database::migrations::migrate(&mut db).unwrap();
        diesel::insert_into(sites::table)
            .values(NewSite {
                name: "legacy",
                updated: 1,
                public_url: "https://symbol.example",
                content_revision: 2,
                tree_hash: "blake3:legacy",
                creator_kind: None,
                creator_hash: None,
                claim_hash: None,
                management_hash: None,
                management_status: 0,
            })
            .execute(&mut db)
            .unwrap();
        let site_id = site_id_locked(&mut db, "legacy").unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq("legacy-hash"),
                blobs::bytes.eq(Vec::<u8>::new()),
                blobs::size.eq(4_i64),
            ))
            .execute(&mut db)
            .unwrap();
        ensure_file_entry(&mut db, site_id, "index.html").unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                site_id,
                path: "index.html",
                hash: "legacy-hash",
                size: 4,
            })
            .execute(&mut db)
            .unwrap();
        database::migrations::downgrade_to_v2(&mut db).unwrap();

        run_migrations(&mut db).unwrap();

        assert_eq!(
            schema_version(&mut db),
            database::schema::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            path_aggregates::table
                .find((site_id, ""))
                .select((path_aggregates::logical_bytes, path_aggregates::file_count))
                .first::<(i64, i64)>(&mut db)
                .unwrap(),
            (4, 1)
        );
        assert_eq!(
            management_audit::table
                .count()
                .get_result::<i64>(&mut db)
                .unwrap(),
            0
        );
    }

    #[test]
    fn path_aggregates_follow_incremental_put_delete_copy_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "assets/a.txt", b"aaa").unwrap();
        store
            .put_file("hello", "assets/nested/b.txt", b"bb")
            .unwrap();
        store.put_file("hello", "root.txt", b"r").unwrap();
        store.put_file("hello", "assets/a.txt", b"aaaaa").unwrap();

        let aggregate = |site: &str, path: &str| {
            let mut db = test_connection(&dir.path().join("symbol.db"));
            let site_id = site_id_locked(&mut db, site).unwrap();
            path_aggregates::table
                .find((site_id, path))
                .select((path_aggregates::logical_bytes, path_aggregates::file_count))
                .first::<(i64, i64)>(&mut db)
                .unwrap()
        };
        assert_eq!(aggregate("hello", ""), (8, 3));
        assert_eq!(aggregate("hello", "assets"), (7, 2));
        assert_eq!(aggregate("hello", "assets/nested"), (2, 1));

        store.delete_file("hello", "assets/nested").unwrap();
        assert_eq!(aggregate("hello", ""), (6, 2));
        assert_eq!(aggregate("hello", "assets"), (5, 1));

        store.copy_site("hello", Some("copy"), None).unwrap();
        assert_eq!(aggregate("copy", ""), (6, 2));
        let token = store.undo_stack("hello").unwrap().entries[0].token.clone();
        store.undo("hello", Some(&token)).unwrap();
        assert_eq!(aggregate("hello", ""), (8, 3));
        assert_eq!(aggregate("hello", "assets/nested"), (2, 1));
    }

    #[test]
    fn undo_is_guarded_bounded_and_keeps_blobs_until_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store.put_file("hello", "index.html", b"first").unwrap();
        let first_hash = match store.lookup("hello", "index.html").unwrap() {
            Node::File { hash, .. } => hash,
            Node::Dir => panic!("expected file"),
        };
        clock.advance(1);
        store.put_file("hello", "index.html", b"second").unwrap();
        let stack = store.undo_stack("hello").unwrap();
        assert_eq!(stack.entries.len(), 2);
        assert!(matches!(
            store.undo("hello", Some(&stack.entries[1].token)),
            Err(StoreError::StaleUndo(_))
        ));
        store.undo("hello", Some(&stack.entries[0].token)).unwrap();
        let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
            panic!("expected restored file");
        };
        assert_eq!(hash, first_hash);

        for index in 0..12 {
            clock.advance(1);
            store
                .put_file("hello", &format!("{index}.txt"), b"value")
                .unwrap();
        }
        assert_eq!(store.undo_stack("hello").unwrap().entries.len(), 10);

        clock.advance(u64::try_from(UNDO_RETENTION_MILLIS).unwrap() + 1);
        store.prune_undo_and_gc().unwrap();
        assert!(store.undo_stack("hello").unwrap().entries.is_empty());
    }

    #[test]
    fn garbage_collection_handles_more_dead_blobs_than_sqlite_variable_limit() {
        const DEAD_BLOBS: usize = 40_000;

        let dir = tempfile::tempdir().unwrap();
        let _store = Store::new(dir.path().to_path_buf()).unwrap();
        let mut db = test_connection(&dir.path().join("symbol.db"));
        let mut tx = DbTransaction::begin(&mut db).unwrap();
        for index in 0..DEAD_BLOBS {
            let hash = format!("{index:064x}");
            diesel::insert_into(blobs::table)
                .values((
                    blobs::hash.eq(hash),
                    blobs::bytes.eq(Vec::<u8>::new()),
                    blobs::size.eq(1_i64),
                ))
                .execute(&mut *tx)
                .unwrap();
        }

        let removed = gc_blobs(&mut tx, 0).unwrap();
        assert_eq!(removed.len(), DEAD_BLOBS);
        assert_eq!(blobs::table.count().get_result::<i64>(&mut *tx).unwrap(), 0);
        tx.commit().unwrap();
    }

    #[test]
    fn pagefind_scale_chunked_sync_remains_writable_after_undo_rotation() {
        const FILES: usize = 42_802;
        const CHUNK_FILES: usize = 4_000;
        const FINAL_DELTA: usize = 1_900;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        for start in (0..FILES).step_by(CHUNK_FILES) {
            let end = (start + CHUNK_FILES).min(FILES);
            let staged = (start..end)
                .map(|index| {
                    stage_bytes(
                        &format!("pagefind/fragment/{index:05}.pf_fragment"),
                        format!("fragment {index}").as_bytes(),
                    )
                })
                .collect::<Vec<_>>();
            store
                .merge_staged("large-sync", &staged, UndoKind::Put)
                .unwrap();
        }
        assert_eq!(
            store.site_inventory("large-sync").unwrap().files.len(),
            FILES
        );

        let final_delta = (FILES - FINAL_DELTA..FILES)
            .map(|index| {
                stage_bytes(
                    &format!("pagefind/fragment/{index:05}.pf_fragment"),
                    format!("updated fragment {index}").as_bytes(),
                )
            })
            .collect::<Vec<_>>();
        store
            .merge_staged("large-sync", &final_delta, UndoKind::Put)
            .unwrap();
        store
            .put_file("large-sync", "pagefind/index.js", b"search index")
            .unwrap();
        assert_eq!(store.undo_stack("large-sync").unwrap().entries.len(), 10);
        assert!(matches!(
            store.lookup("large-sync", "pagefind/index.js"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn unknown_stored_undo_kind_is_rejected_instead_of_mislabeled() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"content").unwrap();
        let token = store.undo_stack("hello").unwrap().entries[0].token.clone();
        let mut db = test_connection(&dir.path().join("symbol.db"));
        diesel::update(undo_operations::table.find(&token))
            .set(undo_operations::kind.eq(999_i64))
            .execute(&mut db)
            .unwrap();
        assert!(matches!(
            store.undo_stack("hello"),
            Err(StoreError::UnsupportedUndoKind(999))
        ));
    }

    #[test]
    fn file_delete_and_site_create_have_isolated_undo_restoration() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("existing", "keep.txt", b"keep").unwrap();
        store.delete_file("existing", "keep.txt").unwrap();
        assert!(matches!(
            store.lookup("existing", "keep.txt"),
            Err(StoreError::NotFound)
        ));
        let delete = store.undo_stack("existing").unwrap().entries[0]
            .token
            .clone();
        store.undo("existing", Some(&delete)).unwrap();
        let hash = match store.lookup("existing", "keep.txt").unwrap() {
            Node::File { hash, .. } => hash,
            Node::Dir => panic!("expected restored file"),
        };
        assert_eq!(store.read_blob(&hash).unwrap().as_ref(), b"keep");

        store.put_file("created", "index.html", b"new").unwrap();
        let create = store.undo_stack("created").unwrap().entries[0]
            .token
            .clone();
        store.undo("created", Some(&create)).unwrap();
        assert!(!store.site_exists("created"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn expiry_inherits_refreshes_copies_moves_sweeps_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store.put_file("hello", "index.html", b"root").unwrap();
        store.put_file("hello", "assets/app.js", b"asset").unwrap();
        store
            .set_expiry(
                "hello",
                "",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 100,
                }),
            )
            .unwrap();
        store
            .set_expiry(
                "hello",
                "assets",
                Some(ExpiryPolicy::Decay(DecayPolicy {
                    min_age_seconds: 50,
                    max_age_seconds: 50,
                    max_size_bytes: 1,
                    power: 1.0,
                })),
            )
            .unwrap();
        store
            .set_expiry(
                "hello",
                "assets/app.js",
                Some(ExpiryPolicy::Absolute {
                    deadline_unix_seconds: 1_700_000_075,
                }),
            )
            .unwrap();

        let report = store.expiry_report("hello", "assets/app.js").unwrap();
        assert_eq!(report.inherited_caps.len(), 2);
        assert_eq!(
            report.effective_expires_at.as_deref(),
            Some("2023-11-14T22:14:10Z")
        );
        assert_eq!(
            report.limited_by,
            Some(ExpiryLimit {
                kind: ExpiryTargetKind::Folder,
                path: Some("assets".to_string()),
            })
        );

        let folder_before = store
            .expiry_report("hello", "assets")
            .unwrap()
            .effective_expires_at;
        clock.advance(10_000);
        store.put_file("hello", "other.txt", b"other").unwrap();
        assert_eq!(
            store
                .expiry_report("hello", "assets")
                .unwrap()
                .effective_expires_at,
            folder_before
        );
        clock.advance(10_000);
        store
            .put_file("hello", "assets/app.js", b"changed")
            .unwrap();
        assert_ne!(
            store
                .expiry_report("hello", "assets")
                .unwrap()
                .effective_expires_at,
            folder_before
        );

        let disabled = store.set_expiry("hello", "assets/app.js", None).unwrap();
        assert!(disabled.report.own_policy.is_none());
        assert!(disabled.report.effective_expires_at.is_some());
        assert_eq!(
            disabled.report.limited_by.unwrap().kind,
            ExpiryTargetKind::Folder
        );

        clock.advance(10_000);
        let (copy, _) = store.copy_site("hello", Some("copy"), None).unwrap();
        let copied_site = store.expiry_report(&copy, "").unwrap();
        assert_eq!(
            copied_site.refreshed_at.as_deref(),
            Some("2023-11-14T22:13:50Z")
        );
        let copied_deadline = copied_site.effective_expires_at;
        store.move_site("copy", "moved").unwrap();
        assert_eq!(
            store
                .expiry_report("moved", "")
                .unwrap()
                .effective_expires_at,
            copied_deadline
        );
        let Node::File { hash, .. } = store.lookup("moved", MANIFEST_PATH).unwrap() else {
            panic!("manifest must exist");
        };
        let manifest = String::from_utf8(store.read_blob(&hash).unwrap().to_vec()).unwrap();
        assert!(manifest.contains("[expiry.site]"));
        assert!(manifest.contains("[expiry.folders.\"assets\"]"));

        store.put_file("soon", "index.html", b"soon").unwrap();
        store
            .set_expiry(
                "soon",
                "index.html",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 1,
                }),
            )
            .unwrap();
        clock.advance(1_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(!store.site_exists("soon"));
        let stack = store.undo_stack("soon").unwrap();
        assert_eq!(stack.entries[0].kind, "expire_sweep");
        store.undo("soon", Some(&stack.entries[0].token)).unwrap();
        assert!(store.site_exists("soon"));
        assert!(matches!(
            store.lookup("soon", "index.html"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn partial_expiry_updates_aggregates_without_full_site_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store.put_file("hello", "assets/a.txt", b"aaa").unwrap();
        store.put_file("hello", "assets/b.txt", b"bb").unwrap();
        store.put_file("hello", "keep.txt", b"k").unwrap();
        store
            .set_expiry(
                "hello",
                "assets",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 1,
                }),
            )
            .unwrap();
        clock.advance(1_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(matches!(
            store.lookup("hello", "keep.txt"),
            Ok(Node::File { .. })
        ));
        assert!(matches!(
            store.lookup("hello", "assets"),
            Err(StoreError::NotFound)
        ));
        let mut db = store.inner.readers.get();
        let site_id = site_id_locked(&mut db, "hello").unwrap();
        let aggregate = path_aggregates::table
            .find((site_id, ""))
            .select((path_aggregates::logical_bytes, path_aggregates::file_count))
            .first::<(i64, i64)>(&mut *db)
            .unwrap();
        assert_eq!(aggregate, (1, 1));
    }

    #[test]
    fn copy_and_move_reuse_blobs_reject_conflicts_and_undo_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("source", "index.html", b"hello").unwrap();
        store.put_file("source", "assets/app.js", b"app").unwrap();
        let source = store.site_inventory("source").unwrap();

        let (_, copied) = store.copy_site("source", Some("copy"), None).unwrap();
        let copy = store.site_inventory("copy").unwrap();
        assert_eq!(copy.tree_hash, source.tree_hash);
        assert_eq!(copy.content_revision, source.content_revision);
        assert_eq!(
            copy.files
                .iter()
                .map(|file| (&file.path, &file.hash))
                .collect::<Vec<_>>(),
            source
                .files
                .iter()
                .map(|file| (&file.path, &file.hash))
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            store.copy_site("source", Some("copy"), None),
            Err(StoreError::DestinationConflict)
        ));

        let (_, moved) = store.move_site("copy", "renamed").unwrap();
        assert!(!store.site_exists("copy"));
        assert_eq!(
            store.site_inventory("renamed").unwrap().tree_hash,
            source.tree_hash
        );
        let Node::File { hash, .. } = store.lookup("renamed", MANIFEST_PATH).unwrap() else {
            panic!("manifest must be a file");
        };
        let manifest = String::from_utf8(store.read_blob(&hash).unwrap().to_vec()).unwrap();
        assert!(manifest.contains("name = \"renamed\""));

        store
            .undo("renamed", Some(&moved.undo.unwrap().token))
            .unwrap();
        assert!(store.site_exists("copy"));
        assert!(!store.site_exists("renamed"));
        store
            .undo("copy", Some(&copied.undo.unwrap().token))
            .unwrap();
        assert!(!store.site_exists("copy"));
        assert!(store.site_exists("source"));
    }

    #[test]
    fn idempotency_replays_generated_resources_and_rejects_fingerprint_changes() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::<TestClock>::clone(&clock),
        )
        .unwrap();
        store.put_file("source", "index.html", b"source").unwrap();
        store.put_file("other", "index.html", b"other").unwrap();
        let idempotency = Idempotency {
            key: "retry-key".to_string(),
        };

        let (first, first_result) = store.copy_site("source", None, Some(&idempotency)).unwrap();
        let (replayed, replayed_result) =
            store.copy_site("source", None, Some(&idempotency)).unwrap();
        assert_eq!(replayed, first);
        assert_eq!(
            replayed_result.undo.unwrap().token,
            first_result.undo.unwrap().token
        );
        assert!(matches!(
            store.copy_site("other", None, Some(&idempotency)),
            Err(StoreError::IdempotencyConflict)
        ));

        clock.advance(u64::try_from(IDEMPOTENCY_RETENTION_MILLIS).unwrap() + 1);
        let (after_expiry, _) = store.copy_site("other", None, Some(&idempotency)).unwrap();
        assert_ne!(after_expiry, first);
    }

    #[test]
    fn unnamed_put_idempotency_replays_without_creating_a_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        let upload = dir.path().join("upload");
        fs::write(&upload, b"first").unwrap();
        let idempotency = Idempotency {
            key: "unnamed-put".to_string(),
        };

        let (first, first_result) = store
            .publish_uploaded_file(
                None,
                "index.html",
                upload.clone(),
                PublishOptions {
                    idempotency: Some(&idempotency),
                    ..PublishOptions::default()
                },
            )
            .unwrap();
        let (replayed, replayed_result) = store
            .publish_uploaded_file(
                None,
                "index.html",
                upload.clone(),
                PublishOptions {
                    idempotency: Some(&idempotency),
                    ..PublishOptions::default()
                },
            )
            .unwrap();
        assert_eq!(replayed, first);
        assert_eq!(
            replayed_result.undo.unwrap().token,
            first_result.undo.unwrap().token
        );
        assert_eq!(store.stats().unwrap().sites, 1);

        fs::write(&upload, b"different").unwrap();
        assert!(matches!(
            store.publish_uploaded_file(
                None,
                "index.html",
                upload,
                PublishOptions {
                    idempotency: Some(&idempotency),
                    ..PublishOptions::default()
                }
            ),
            Err(StoreError::IdempotencyConflict)
        ));
        assert_eq!(store.stats().unwrap().sites, 1);
    }

    #[test]
    fn inventory_and_conditional_put_abort_strictly_on_drift() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"first").unwrap();
        let baseline = store.site_inventory("hello").unwrap();
        assert_eq!(baseline.files.len(), 1);
        assert_eq!(baseline.files[0].path, "index.html");
        assert_eq!(
            baseline.files[0].hash,
            format!("blake3:{}", blake3::hash(b"first").to_hex())
        );

        let update = dir.path().join("update");
        fs::write(&update, b"second").unwrap();
        let changed = store
            .put_uploaded_file("hello", "index.html", update, Some(&baseline.tree_hash))
            .unwrap();
        assert_eq!(changed.revision, baseline.content_revision + 1);

        let rejected = dir.path().join("rejected");
        fs::write(&rejected, b"must not publish").unwrap();
        let error = store
            .put_uploaded_file("hello", "new.txt", rejected, Some(&baseline.tree_hash))
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::PreconditionFailed { revision, .. } if revision == changed.revision
        ));
        assert!(matches!(
            store.lookup("hello", "new.txt"),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn sqlite_index_and_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
            .unwrap();
        assert_eq!(
            store
                .list_sites()
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["hello".to_string()]
        );
        assert_eq!(
            store.list_files("hello").unwrap(),
            vec!["index.html".to_string(), "symbol.toml".to_string()]
        );
        assert!(dir.path().join("symbol.db").is_file());
        let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
            panic!("expected file");
        };
        let blob_path = store.blob_path(&hash);
        assert_eq!(fs::read(&blob_path).unwrap(), b"<h1>x</h1>");
        let mut db = test_connection(&dir.path().join("symbol.db"));
        let stored_bytes = blobs::table
            .find(&hash)
            .select(blobs::bytes)
            .first::<Vec<u8>>(&mut db)
            .unwrap();
        assert!(stored_bytes.is_empty());
        drop(db);
        assert_eq!(store.read_blob(&hash).unwrap().as_ref(), b"<h1>x</h1>");
        let tar = store.pack_site("hello", ArchiveFormat::Tar).unwrap();
        assert_eq!(&tar[257..262], b"ustar");
        let tar_gz = store.pack_site("hello", ArchiveFormat::TarGz).unwrap();
        assert_eq!(&tar_gz[..2], [0x1f, 0x8b]);
        let zip = store.pack_site("hello", ArchiveFormat::Zip).unwrap();
        assert_eq!(&zip[..4], b"PK\x03\x04");
        assert_eq!(store.list_sites().unwrap().entries.len(), 1);
        let packed = store.pop_site("hello").unwrap();
        assert_eq!(&packed[..2], [0x1f, 0x8b]);
        assert!(store.list_sites().unwrap().entries.is_empty());
        assert_eq!(store.read_blob(&hash).unwrap().as_ref(), b"<h1>x</h1>");
        assert!(blob_path.exists());
    }

    #[test]
    fn startup_migrates_sqlite_blob_payloads_to_files() {
        let dir = tempfile::tempdir().unwrap();
        let hash = blake3::hash(b"legacy").to_hex().to_string();
        {
            let mut db = test_connection(&dir.path().join("symbol.db"));
            run_migrations(&mut db).unwrap();
            diesel::insert_into(blobs::table)
                .values((
                    blobs::hash.eq(&hash),
                    blobs::bytes.eq(b"legacy".as_slice()),
                    blobs::size.eq(6_i64),
                ))
                .execute(&mut db)
                .unwrap();
            let site_id = diesel::insert_into(sites::table)
                .values(NewSite {
                    name: "hello",
                    updated: 0,
                    public_url: "",
                    content_revision: 0,
                    tree_hash: "",
                    creator_kind: None,
                    creator_hash: None,
                    claim_hash: None,
                    management_hash: None,
                    management_status: 0,
                })
                .returning(sites::id)
                .get_result::<i64>(&mut db)
                .unwrap();
            ensure_file_entry(&mut db, site_id, "legacy.bin").unwrap();
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id,
                    path: "legacy.bin",
                    hash: &hash,
                    size: 6,
                })
                .execute(&mut db)
                .unwrap();
        }
        let target = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"broken").unwrap();

        let store = Store::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(fs::read(store.blob_path(&hash)).unwrap(), b"legacy");
        assert_eq!(store.read_blob(&hash).unwrap(), "legacy");
        let mut db = test_connection(&dir.path().join("symbol.db"));
        assert_eq!(
            blobs::table
                .find(&hash)
                .select(blobs::bytes)
                .first::<Vec<u8>>(&mut db)
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            metadata::table
                .find("external_blobs_v1")
                .select(metadata::value)
                .first::<String>(&mut db)
                .unwrap(),
            "1"
        );
    }

    #[test]
    fn startup_never_deletes_unreferenced_blob_files() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::new(dir.path().to_path_buf()).unwrap());
        let orphan_hash = "aa00000000000000000000000000000000000000000000000000000000000000";
        let orphan = dir
            .path()
            .join("blobs")
            .join(&orphan_hash[..2])
            .join(&orphan_hash[2..]);
        fs::create_dir_all(orphan.parent().unwrap()).unwrap();
        fs::write(&orphan, b"orphan").unwrap();

        drop(Store::new(dir.path().to_path_buf()).unwrap());
        assert_eq!(fs::read(orphan).unwrap(), b"orphan");
    }

    #[test]
    fn startup_restores_catalog_references_from_blob_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let hash;
        {
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store.put_file("hello", "live.bin", b"live").unwrap();
            let Node::File {
                hash: stored_hash, ..
            } = store.lookup("hello", "live.bin").unwrap()
            else {
                panic!("expected file");
            };
            hash = stored_hash;
        }
        let live = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
        let quarantined = dir
            .path()
            .join("blobs")
            .join(".quarantine")
            .join(&hash[..2])
            .join(&hash[2..]);
        fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
        fs::rename(&live, &quarantined).unwrap();

        let store = Store::new(dir.path().to_path_buf()).unwrap();

        assert_eq!(store.read_blob(&hash).unwrap().as_ref(), b"live");
        assert!(live.exists());
        assert!(!quarantined.exists());
    }

    #[test]
    fn put_file_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
            .unwrap();
        let err = store
            .put_file("hello", "._index.html", &[0x00, 0x05, 0x16, 0x07])
            .unwrap_err();
        assert!(matches!(err, StoreError::Upload(UploadError::Junk)));
        assert_eq!(
            store.list_files("hello").unwrap(),
            vec!["index.html".to_string(), "symbol.toml".to_string()]
        );
    }

    #[test]
    fn startup_does_not_delete_preexisting_junk_content() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store
                .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
                .unwrap();
        }
        {
            let mut db = test_connection(&dir.path().join("symbol.db"));
            let apple = [0x00u8, 0x05, 0x16, 0x07, 0, 2, 0, 0];
            let apple_len = i64::try_from(apple.len()).unwrap();
            let hash = blake3::hash(&apple).to_hex().to_string();
            let blob = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
            fs::create_dir_all(blob.parent().unwrap()).unwrap();
            fs::write(blob, apple).unwrap();
            diesel::insert_into(blobs::table)
                .values((
                    blobs::hash.eq(&hash),
                    blobs::bytes.eq(Vec::<u8>::new()),
                    blobs::size.eq(apple_len),
                ))
                .execute(&mut db)
                .unwrap();
            let site_id = site_id_locked(&mut db, "hello").unwrap();
            for path in ["._index.html", "keep.bin"] {
                ensure_file_entry(&mut db, site_id, path).unwrap();
                diesel::insert_into(files::table)
                    .values(NewFile {
                        site_id,
                        path,
                        hash: &hash,
                        size: apple_len,
                    })
                    .execute(&mut db)
                    .unwrap();
            }
        }
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            store.list_files("hello").unwrap(),
            vec![
                "._index.html".to_string(),
                "index.html".to_string(),
                "keep.bin".to_string(),
                "symbol.toml".to_string(),
            ]
        );
    }

    #[test]
    fn distributions_cover_empty_single_even_and_odd_populations() {
        let empty = distribution(&[]);
        assert!(empty.min.is_none());
        assert!(empty.mean.is_none());
        assert!(empty.stddev.is_none());

        let single = distribution(&[7]);
        assert_eq!(single.min, Some(7));
        assert_eq!(single.median, Some(7.0));
        assert_eq!(single.stddev, Some(0.0));

        let even = distribution(&[1, 2]);
        assert_eq!(even.p25, Some(1.25));
        assert_eq!(even.median, Some(1.5));
        assert_eq!(even.p75, Some(1.75));
        assert_eq!(even.stddev, Some(0.5));

        let odd = distribution(&[1, 2, 3]);
        assert_eq!(odd.p25, Some(1.5));
        assert_eq!(odd.median, Some(2.0));
        assert_eq!(odd.p75, Some(2.5));
        assert!((odd.stddev.unwrap() - (2.0_f64 / 3.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn stats_report_cross_site_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("one", "a.txt", b"same").unwrap();
        store.put_file("two", "b.txt", b"same").unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.sites, 2);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.blobs, 1);
        assert_eq!(stats.bytes, 4);
        assert_eq!(stats.logical_bytes, 8);
        assert_eq!(stats.saved_bytes, 4);
        assert!((stats.saved_fraction - 0.5).abs() < f64::EPSILON);
        assert_eq!(stats.file_sizes.median, Some(4.0));
        assert_eq!(stats.blob_sizes.median, Some(4.0));
    }

    #[test]
    fn listings_include_recursive_file_counts_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "a/x.bin", &[1; 10]).unwrap();
        store.put_file("hello", "a/y.bin", &[2; 20]).unwrap();
        store.put_file("hello", "b/z.bin", &[3; 7]).unwrap();
        store.put_file("hello", "root.bin", &[4; 5]).unwrap();

        let root = store.list_dir("hello", "").unwrap();
        assert_eq!(root.files, 5);
        assert!(root.bytes > 42);
        assert_eq!(root.entries.len(), 4);
        assert_eq!(root.entries[0].kind, EntryKind::Directory);
        assert_eq!(root.entries[0].name, "a");
        assert_eq!(root.entries[0].files, 2);
        assert_eq!(root.entries[0].bytes, 30);
        assert_eq!(root.entries[1].name, "b");
        assert_eq!(root.entries[1].files, 1);
        assert_eq!(root.entries[1].bytes, 7);
        assert_eq!(root.entries[2].kind, EntryKind::File);
        assert_eq!(root.entries[2].name, "root.bin");
        assert_eq!(root.entries[2].bytes, 5);

        let nested = store.list_dir("hello", "a").unwrap();
        assert_eq!(nested.files, 2);
        assert_eq!(nested.bytes, 30);

        let sites = store.list_sites().unwrap();
        assert_eq!(sites.files, 4);
        assert_eq!(sites.bytes, 42);
        assert_eq!(sites.entries[0].files, 4);
        assert_eq!(sites.entries[0].bytes, 42);
    }

    #[test]
    fn lookup_distinguishes_files_directories_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "docs/index.html", b"docs").unwrap();

        assert!(matches!(store.lookup("hello", ""), Ok(Node::Dir)));
        assert!(matches!(store.lookup("hello", "docs"), Ok(Node::Dir)));
        let Node::File { logical, hash } = store.lookup("hello", "docs/index.html").unwrap() else {
            panic!("expected file");
        };
        assert_eq!(logical, "docs/index.html");
        assert_eq!(hash, blake3::hash(b"docs").to_hex().as_str());
        assert!(matches!(
            store.lookup("hello", "missing"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.lookup("absent", ""),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn nested_listing_and_delete_use_literal_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "a%/one.txt", b"one").unwrap();
        store.put_file("hello", "a_/two.txt", b"two").unwrap();
        store.put_file("hello", "a0/three.txt", b"three").unwrap();

        let listing = store.list_dir("hello", "a%").unwrap();
        assert_eq!(listing.files, 1);
        assert_eq!(listing.entries[0].name, "one.txt");

        store.delete_file("hello", "a%").unwrap();
        assert!(matches!(
            store.lookup("hello", "a%/one.txt"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.lookup("hello", "a_/two.txt"),
            Ok(Node::File { .. })
        ));
        assert!(matches!(
            store.lookup("hello", "a0/three.txt"),
            Ok(Node::File { .. })
        ));
    }

    #[test]
    fn blob_cache_is_byte_bounded_and_evicts_least_recently_used() {
        let cache = BlobCache::new(266, 16, Arc::new(Metrics::default()));
        cache.insert("a", Bytes::from_static(b"aaa"));
        cache.insert("b", Bytes::from_static(b"bbb"));
        assert_eq!(cache.get("a").unwrap(), "aaa");

        cache.insert("c", Bytes::from_static(b"ccc"));

        assert!(cache.contains("a"));
        assert!(!cache.contains("b"));
        assert!(cache.contains("c"));
        let state = cache.state.lock().unwrap();
        assert!(state.charge <= cache.capacity);
        assert_eq!(state.recency.len(), state.entries.len());
    }

    #[test]
    fn blob_cache_caps_entry_count() {
        let cache = BlobCache::new(usize::MAX, 2, Arc::new(Metrics::default()));
        cache.insert("a", Bytes::new());
        cache.insert("b", Bytes::new());
        cache.insert("c", Bytes::new());

        assert!(!cache.contains("a"));
        assert!(cache.contains("b"));
        assert!(cache.contains("c"));
    }

    #[test]
    fn serving_metrics_count_cache_activity_and_reader_waits() {
        let metrics = Arc::new(Metrics::default());
        let cache = BlobCache::new(1024, 1, Arc::clone(&metrics));
        assert!(cache.get("missing").is_none());
        cache.insert("a", Bytes::from_static(b"a"));
        assert_eq!(cache.get("a").unwrap(), "a");
        cache.insert("b", Bytes::from_static(b"b"));

        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        let held: Vec<_> = (0..store.inner.readers.size())
            .map(|_| store.inner.readers.get())
            .collect();
        let concurrent = store.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            concurrent.list_sites().unwrap();
        });
        started_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(held);
        thread.join().unwrap();

        let cache_stats = metrics.snapshot().cache;
        assert_eq!(cache_stats.hits, 1);
        assert_eq!(cache_stats.misses, 1);
        assert_eq!(cache_stats.evictions, 1);
        let reader_stats = store.inner.metrics.snapshot().readers;
        assert_eq!(reader_stats.waits, 1);
        assert!(reader_stats.operations >= 1);
        assert!(reader_stats.wait_micros > 0);
    }

    #[test]
    fn repeated_blob_reads_share_cached_storage_and_gc_evicts_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"first").unwrap();
        let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
            panic!("expected file");
        };

        let first = store.read_blob(&hash).unwrap();
        let second = store.read_blob(&hash).unwrap();
        assert_eq!(first.as_ptr(), second.as_ptr());
        assert!(store.inner.blobs.contains(&hash));

        store.put_file("hello", "index.html", b"second").unwrap();
        assert!(store.inner.blobs.contains(&hash));
        assert_eq!(store.read_blob(&hash).unwrap(), "first");
    }

    #[test]
    fn reader_pool_serves_another_query_while_one_reader_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"hello").unwrap();
        let held = store.inner.readers.get();
        let concurrent = store.clone();
        let (sent, received) = std::sync::mpsc::channel();

        let thread = std::thread::spawn(move || {
            sent.send(concurrent.list_sites().unwrap()).unwrap();
        });
        let sites = received
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second reader should not wait for the first");

        assert_eq!(sites.entries[0].name, "hello");
        drop(held);
        thread.join().unwrap();
    }

    #[test]
    fn concurrent_reads_and_disjoint_writes_remain_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"hello").unwrap();
        let start = Arc::new(std::sync::Barrier::new(9));

        std::thread::scope(|scope| {
            for writer in 0..4 {
                let store = store.clone();
                let start = Arc::clone(&start);
                scope.spawn(move || {
                    start.wait();
                    for file in 0..10 {
                        store
                            .put_file("hello", &format!("writer-{writer}/{file}.txt"), b"value")
                            .unwrap();
                    }
                });
            }
            for _ in 0..4 {
                let store = store.clone();
                let start = Arc::clone(&start);
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..50 {
                        assert!(matches!(
                            store.lookup("hello", "index.html"),
                            Ok(Node::File { .. })
                        ));
                        assert!(store.list_dir("hello", "").unwrap().files >= 1);
                    }
                });
            }
            start.wait();
        });

        assert_eq!(store.list_files("hello").unwrap().len(), 42);
    }

    #[test]
    fn concurrent_upload_paths_are_unique_within_one_clock_tick() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(dir.path().to_path_buf(), "http://symbol".to_string(), clock)
            .unwrap();
        let paths = Arc::new(Mutex::new(HashSet::new()));
        std::thread::scope(|scope| {
            for _ in 0..32 {
                let store = store.clone();
                let paths = Arc::clone(&paths);
                scope.spawn(move || {
                    paths.lock().unwrap().insert(store.upload_path());
                });
            }
        });
        assert_eq!(paths.lock().unwrap().len(), 32);
    }

    fn node_hash(store: &Store, site: &str, path: &str) -> String {
        let Node::File { hash, .. } = store.lookup(site, path).unwrap() else {
            panic!("expected file");
        };
        hash
    }

    #[test]
    fn single_file_mutation_on_42802_file_site_stores_one_delta() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("large", "index.html", b"old").unwrap();
        let hash = node_hash(&store, "large", "index.html");
        {
            let mut db = store.inner.writer.lock().unwrap();
            let site_id = site_id_locked(&mut db, "large").unwrap();
            diesel::sql_query(format!(
                "WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x + 1 FROM n WHERE x < 42800)
                 INSERT INTO site_entries(site_id, path, kind)
                 SELECT {site_id}, printf('bulk/%05d', x), 0 FROM n"
            ))
            .execute(&mut *db)
            .unwrap();
            diesel::sql_query(format!(
                "INSERT INTO files(site_id, path, kind, hash, size)
                 SELECT site_id, path, 0, '{hash}', 3
                 FROM site_entries WHERE site_id = {site_id} AND path LIKE 'bulk/%'"
            ))
            .execute(&mut *db)
            .unwrap();
        }
        store.put_file("large", "index.html", b"new").unwrap();
        let mut db = test_connection(&dir.path().join("symbol.db"));
        let token = undo_operations::table
            .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
            .filter(undo_names::name.eq("large"))
            .select(undo_operations::token)
            .order(undo_operations::rowid.desc())
            .first::<String>(&mut db)
            .unwrap();
        assert_eq!(
            undo_file_deltas::table
                .filter(undo_file_deltas::token.eq(&token))
                .select(count_star())
                .first::<i64>(&mut db)
                .unwrap(),
            1
        );
        assert_eq!(
            undo_files::table
                .filter(undo_files::token.eq(&token))
                .select(count_star())
                .first::<i64>(&mut db)
                .unwrap(),
            0
        );
    }

    #[test]
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    fn allocated_names_are_exact_idempotent_and_relocate_with_undo() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("assets", "index.html", b"site").unwrap();
        let idempotency = Idempotency {
            key: "allocated-once".to_string(),
        };
        let options = FileMutationOptions {
            idempotency: Some(&idempotency),
            ..FileMutationOptions::default()
        };
        let first = store
            .allocate_bytes(
                "assets",
                b"payload",
                AllocationSpec {
                    folder: "nested/assets",
                    naming: AllocatedName {
                        prefix: "pre-",
                        suffix: "-final",
                        extension: Some("..JP G.."),
                    },
                    media_type: "IMAGE/JPEG",
                },
                options,
            )
            .unwrap();
        assert_eq!(
            first.path,
            format!(
                "nested/assets/pre-{}-final.jp-g",
                blake3::hash(b"payload").to_hex()
            )
        );
        assert_eq!(store.read_blob(&first.hash).unwrap().as_ref(), b"payload");
        let first_metadata = store.allocated_metadata("assets", &first.path).unwrap();
        assert_eq!(
            first_metadata.naming_mode,
            AllocatedNamingMode::ContentAddressed
        );
        assert_eq!(first_metadata.prefix, "pre-");
        assert_eq!(first_metadata.suffix, "-final");
        assert_eq!(first_metadata.extension.as_deref(), Some("jp-g"));
        assert_eq!(first_metadata.media_type, "image/jpeg");
        store
            .copy_site("assets", Some("assets-copy"), None)
            .unwrap();
        assert_eq!(
            store
                .allocated_metadata("assets-copy", &first.path)
                .unwrap(),
            first_metadata
        );
        let replay = store
            .allocate_bytes(
                "assets",
                b"payload",
                AllocationSpec {
                    folder: "nested/assets",
                    naming: AllocatedName {
                        prefix: "pre-",
                        suffix: "-final",
                        extension: Some("..JP G.."),
                    },
                    media_type: "image/jpeg",
                },
                options,
            )
            .unwrap();
        assert!(replay.replayed);
        let source = dir.path().join("from-file.bin");
        fs::write(&source, b"file payload").unwrap();
        let from_file = store
            .allocate_file(
                "assets",
                &source,
                AllocationSpec {
                    folder: "",
                    naming: AllocatedName {
                        prefix: "file-",
                        suffix: "",
                        extension: Some("bin"),
                    },
                    media_type: "application/octet-stream",
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            store.read_blob(&from_file.hash).unwrap().as_ref(),
            b"file payload"
        );
        store
            .set_expiry(
                "assets",
                &first.path,
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 600,
                }),
            )
            .unwrap();
        let moved = store
            .replace_allocated(
                "assets",
                &first.path,
                AllocationSource::Bytes(b"changed"),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_ne!(moved.path, first.path);
        let moved_metadata = store.allocated_metadata("assets", &moved.path).unwrap();
        assert_eq!(moved_metadata.prefix, first_metadata.prefix);
        assert_eq!(moved_metadata.suffix, first_metadata.suffix);
        assert_eq!(moved_metadata.extension, first_metadata.extension);
        assert_eq!(moved_metadata.media_type, first_metadata.media_type);
        assert!(matches!(
            store.lookup("assets", &first.path),
            Err(StoreError::NotFound)
        ));
        store.expiry_report("assets", &moved.path).unwrap();
        assert!(store.blob_path(&first.hash).is_file());
        store.undo("assets", None).unwrap();
        assert_eq!(node_hash(&store, "assets", &first.path), first.hash);
        assert_eq!(
            store.allocated_metadata("assets", &first.path).unwrap(),
            first_metadata
        );
        store.expiry_report("assets", &first.path).unwrap();
        assert!(matches!(
            store.lookup("assets", &moved.path),
            Err(StoreError::NotFound)
        ));
        store.delete_file("assets", &first.path).unwrap();
        assert!(store.blob_path(&first.hash).is_file());
        store.undo("assets", None).unwrap();
        assert_eq!(node_hash(&store, "assets", &first.path), first.hash);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn allocation_replacement_and_splice_redact_before_hashing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("sanitize-mutations", "regular.txt", b"before")
            .unwrap();
        store
            .put_file("sanitize-mutations", "splice.txt", b"prefix:")
            .unwrap();
        let management = format!("sym_mgmt_{}", "a".repeat(64));
        let claim = format!("sym_claim_{}", "b".repeat(64));
        let redacted_management = format!("sym_mgmt_{}", "*".repeat(64));
        let redacted_claim = format!("sym_claim_{}", "*".repeat(64));
        let allocation_input = format!("{management}\n{claim}");
        let allocation_output = format!("{redacted_management}\n{redacted_claim}");

        let allocated = store
            .allocate_bytes(
                "sanitize-mutations",
                allocation_input.as_bytes(),
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            allocated.mutation.as_ref().unwrap().sanitized,
            TokenCounts {
                management: 1,
                claim: 1
            }
        );
        assert_eq!(
            allocated.hash,
            blake3::hash(allocation_output.as_bytes())
                .to_hex()
                .to_string()
        );
        assert!(allocated.path.contains(&allocated.hash));
        assert_eq!(
            store.read_blob(&allocated.hash).unwrap().as_ref(),
            allocation_output.as_bytes()
        );

        let proposal = store
            .propose_allocation(
                "sanitize-mutations",
                AllocationSource::Bytes(management.as_bytes()),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        assert_eq!(
            proposal.hash,
            blake3::hash(redacted_management.as_bytes())
                .to_hex()
                .to_string()
        );
        let finalized = store
            .finalize_allocation_custom(
                "sanitize-mutations",
                &proposal.token,
                "pending.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            finalized.mutation.as_ref().unwrap().sanitized,
            TokenCounts {
                management: 1,
                claim: 0
            }
        );

        let regular_hash = node_hash(&store, "sanitize-mutations", "regular.txt");
        let replaced = store
            .replace_file_content(
                "sanitize-mutations",
                "regular.txt",
                &regular_hash,
                AllocationSource::Bytes(claim.as_bytes()),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            replaced.mutation.as_ref().unwrap().sanitized,
            TokenCounts {
                management: 0,
                claim: 1
            }
        );
        assert_eq!(
            store.read_blob(&replaced.hash).unwrap().as_ref(),
            redacted_claim.as_bytes()
        );

        let splice_hash = node_hash(&store, "sanitize-mutations", "splice.txt");
        let spliced = store
            .splice_file(
                "sanitize-mutations",
                "splice.txt",
                &splice_hash,
                &[Splice {
                    offset: 7,
                    delete: 0,
                    insert: SpliceSource::Bytes(management.as_bytes()),
                }],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            spliced.mutation.as_ref().unwrap().sanitized,
            TokenCounts {
                management: 1,
                claim: 0
            }
        );
        assert_eq!(
            store.read_blob(&spliced.hash).unwrap().as_ref(),
            format!("prefix:{redacted_management}").as_bytes()
        );
    }

    #[test]
    fn allocated_extension_normalization_matches_shared_sdk_vectors() {
        for vector in symbol_contract::EXTENSION_NORMALIZATION_VECTORS {
            assert_eq!(
                normalize_allocated_extension(vector.input).ok().as_deref(),
                vector.output,
                "{:?}",
                vector.input
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn pending_allocation_finalizes_once_and_pruning_releases_blob() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::clone(&clock) as Arc<dyn Clock>,
        )
        .unwrap();
        store.put_file("pending", "index.html", b"site").unwrap();
        let proposal = store
            .propose_allocation(
                "pending",
                AllocationSource::Bytes(b"one"),
                PendingAllocationSpec {
                    folder: "nested/pending",
                    media_type: "application/custom",
                    extension: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(proposal.size, 3);
        assert_eq!(proposal.folder, "nested/pending");
        assert_eq!(proposal.media_type, "application/custom");
        assert!(proposal.expires_at.ends_with('Z'));
        store.put_file("other", "index.html", b"other").unwrap();
        assert!(matches!(
            store.finalize_allocation(
                "other",
                &proposal.token,
                AllocatedName::default(),
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidPendingAllocation)
        ));
        let mut db = test_connection(&dir.path().join("symbol.db"));
        assert_eq!(
            blobs::table
                .filter(blobs::hash.eq(&proposal.hash))
                .select(count_star())
                .first::<i64>(&mut db)
                .unwrap(),
            1
        );
        drop(db);
        let finalized = store
            .finalize_allocation(
                "pending",
                &proposal.token,
                AllocatedName {
                    prefix: "custom-",
                    suffix: "",
                    extension: Some("BIN"),
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(finalized.hash, proposal.hash);
        assert!(finalized.path.starts_with("nested/pending/custom-"));
        let mut db = test_connection(&dir.path().join("symbol.db"));
        assert_eq!(
            blobs::table
                .filter(blobs::hash.eq(&proposal.hash))
                .select(count_star())
                .first::<i64>(&mut db)
                .unwrap(),
            1
        );
        drop(db);
        assert!(matches!(
            store.finalize_allocation(
                "pending",
                &proposal.token,
                AllocatedName::default(),
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidPendingAllocation)
        ));

        let abandoned = store
            .propose_allocation(
                "pending",
                AllocationSource::Bytes(b"abandoned"),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        assert!(store.blob_path(&abandoned.hash).is_file());
        clock.advance(u64::try_from(PENDING_RETENTION_MILLIS).unwrap() + 1);
        assert_eq!(store.prune_pending_allocations().unwrap(), 1);
        assert!(!store.blob_path(&abandoned.hash).exists());
        assert!(
            dir.path()
                .join("blobs/.quarantine")
                .join(&abandoned.hash[..2])
                .join(&abandoned.hash[2..])
                .is_file()
        );

        let cancelled = store
            .propose_allocation(
                "pending",
                AllocationSource::Bytes(b"cancelled"),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        store
            .cancel_allocation("pending", &cancelled.token, None)
            .unwrap();
        assert!(!store.blob_path(&cancelled.hash).exists());
        assert!(
            dir.path()
                .join("blobs/.quarantine")
                .join(&cancelled.hash[..2])
                .join(&cancelled.hash[2..])
                .is_file()
        );
    }

    #[test]
    fn custom_finalize_validates_basename_and_reports_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("custom", "index.html", b"site").unwrap();
        let proposal = store
            .propose_allocation(
                "custom",
                AllocationSource::Bytes(b"custom payload"),
                PendingAllocationSpec {
                    folder: "nested/custom",
                    media_type: "text/plain",
                    extension: None,
                },
                None,
            )
            .unwrap();
        for invalid in ["", ".", "..", "../escape", "a/b", r"a\b", "UNDO"] {
            assert!(matches!(
                store.finalize_allocation_custom(
                    "custom",
                    &proposal.token,
                    invalid,
                    FileMutationOptions::default()
                ),
                Err(StoreError::InvalidAllocatedName)
            ));
        }
        let finalized = store
            .finalize_allocation_custom(
                "custom",
                &proposal.token,
                "safe-name.txt",
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(finalized.path, "nested/custom/safe-name.txt");
        let metadata = store.allocated_metadata("custom", &finalized.path).unwrap();
        assert_eq!(metadata.naming_mode, AllocatedNamingMode::Custom);
        assert_eq!(metadata.media_type, "text/plain");

        store
            .put_file("custom", "nested/custom/taken.txt", b"taken")
            .unwrap();
        let conflict = store
            .propose_allocation(
                "custom",
                AllocationSource::Bytes(b"conflict"),
                PendingAllocationSpec {
                    folder: "nested/custom",
                    media_type: "text/plain",
                    extension: None,
                },
                None,
            )
            .unwrap();
        assert!(matches!(
            store.finalize_allocation_custom(
                "custom",
                &conflict.token,
                "taken.txt",
                FileMutationOptions::default()
            ),
            Err(StoreError::DestinationConflict)
        ));
        store
            .cancel_allocation("custom", &conflict.token, None)
            .unwrap();
    }

    #[test]
    fn allocated_expiry_sweep_and_whole_site_undo_keep_blob_reachable() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::clone(&clock) as Arc<dyn Clock>,
        )
        .unwrap();
        store
            .put_file("expiry-allocated", "index.html", b"site")
            .unwrap();
        let allocated = store
            .allocate_bytes(
                "expiry-allocated",
                b"expires",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "expiry-allocated",
                &allocated.path,
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 1,
                }),
            )
            .unwrap();
        clock.advance(1_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(matches!(
            store.lookup("expiry-allocated", &allocated.path),
            Err(StoreError::NotFound)
        ));
        assert!(store.blob_path(&allocated.hash).is_file());
        store.undo("expiry-allocated", None).unwrap();
        assert_eq!(
            node_hash(&store, "expiry-allocated", &allocated.path),
            allocated.hash
        );
    }

    #[test]
    fn replacement_requires_current_hash_and_preserves_regular_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("replace", "data.bin", b"old").unwrap();
        let old_hash = node_hash(&store, "replace", "data.bin");
        assert!(matches!(
            store.replace_file_content(
                "replace",
                "data.bin",
                "stale",
                AllocationSource::Bytes(b"new"),
                FileMutationOptions::default()
            ),
            Err(StoreError::StaleContentHash(current)) if current == old_hash
        ));
        let replaced = store
            .replace_file_content(
                "replace",
                "data.bin",
                &old_hash,
                AllocationSource::Bytes(b"new"),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(replaced.path, "data.bin");
        store.undo("replace", None).unwrap();
        assert_eq!(node_hash(&store, "replace", "data.bin"), old_hash);
    }

    #[test]
    fn multi_splice_uses_original_offsets_and_validates_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("splice", "data.txt", b"0123456789").unwrap();
        let base = node_hash(&store, "splice", "data.txt");
        let result = store
            .splice_file(
                "splice",
                "data.txt",
                &base,
                &[
                    Splice {
                        offset: 0,
                        delete: 0,
                        insert: SpliceSource::Bytes(b"A"),
                    },
                    Splice {
                        offset: 2,
                        delete: 3,
                        insert: SpliceSource::Bytes(b"BC"),
                    },
                    Splice {
                        offset: 10,
                        delete: 0,
                        insert: SpliceSource::Bytes(b"Z"),
                    },
                ],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            store.read_blob(&result.hash).unwrap().as_ref(),
            b"A01BC56789Z"
        );
        assert!(matches!(
            store.splice_file(
                "splice",
                "data.txt",
                &result.hash,
                &[
                    Splice {
                        offset: 4,
                        delete: 2,
                        insert: SpliceSource::Empty,
                    },
                    Splice {
                        offset: 5,
                        delete: 0,
                        insert: SpliceSource::Empty,
                    }
                ],
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidSpliceOrder)
        ));
        assert!(matches!(
            store.splice_file(
                "splice",
                "data.txt",
                &result.hash,
                &[Splice {
                    offset: 99,
                    delete: 0,
                    insert: SpliceSource::Empty,
                }],
                FileMutationOptions::default()
            ),
            Err(StoreError::SpliceRange)
        ));
        let secret = format!("sym_mgmt_{}\n", "a".repeat(64));
        let sanitized = store
            .splice_file(
                "splice",
                "data.txt",
                &result.hash,
                &[Splice {
                    offset: 0,
                    delete: 0,
                    insert: SpliceSource::Bytes(secret.as_bytes()),
                }],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(sanitized.mutation.unwrap().sanitized.management, 1);
        assert_eq!(
            store.read_blob(&sanitized.hash).unwrap().as_ref(),
            format!("sym_mgmt_{}\nA01BC56789Z", "*".repeat(64)).as_bytes()
        );
    }

    #[test]
    fn splice_streams_large_file_and_file_insertions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        let source = dir.path().join("large.bin");
        let insertion = dir.path().join("insert.bin");
        {
            let mut file = fs::File::create(&source).unwrap();
            for _ in 0..256 {
                file.write_all(&[7_u8; 64 * 1024]).unwrap();
            }
        }
        fs::write(&insertion, [8_u8; 64 * 1024]).unwrap();
        store
            .put_uploaded_file("stream", "large.bin", source, None)
            .unwrap();
        let base = node_hash(&store, "stream", "large.bin");
        let result = store
            .splice_file(
                "stream",
                "large.bin",
                &base,
                &[Splice {
                    offset: 8 * 1024 * 1024,
                    delete: 64 * 1024,
                    insert: SpliceSource::File(&insertion),
                }],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(result.size, 16 * 1024 * 1024);
        let mut file = fs::File::open(store.blob_path(&result.hash)).unwrap();
        file.seek(SeekFrom::Start(8 * 1024 * 1024)).unwrap();
        let mut marker = [0_u8; 1];
        file.read_exact(&mut marker).unwrap();
        assert_eq!(marker, [8]);
    }

    #[test]
    fn writer_transaction_rechecks_replace_and_splice_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("cas", "data.txt", b"base").unwrap();
        let base = node_hash(&store, "cas", "data.txt");
        let racer = store.clone();
        store.set_before_content_commit(move || {
            racer.put_file("cas", "data.txt", b"racer").unwrap();
        });
        assert!(matches!(
            store.replace_file_content(
                "cas",
                "data.txt",
                &base,
                AllocationSource::Bytes(b"replacement"),
                FileMutationOptions::default(),
            ),
            Err(StoreError::StaleContentHash(current))
                if current == blake3::hash(b"racer").to_hex().as_str()
        ));
        assert_eq!(
            store
                .read_blob(&node_hash(&store, "cas", "data.txt"))
                .unwrap()
                .as_ref(),
            b"racer"
        );

        let splice_base = node_hash(&store, "cas", "data.txt");
        let racer = store.clone();
        store.set_before_content_commit(move || {
            racer.put_file("cas", "data.txt", b"second racer").unwrap();
        });
        assert!(matches!(
            store.splice_file(
                "cas",
                "data.txt",
                &splice_base,
                &[Splice {
                    offset: 0,
                    delete: 0,
                    insert: SpliceSource::Bytes(b"x"),
                }],
                FileMutationOptions::default(),
            ),
            Err(StoreError::StaleContentHash(current))
                if current == blake3::hash(b"second racer").to_hex().as_str()
        ));

        let pending = store
            .propose_allocation(
                "cas",
                AllocationSource::Bytes(b"allocated base"),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        let allocated = store
            .finalize_allocation_custom(
                "cas",
                &pending.token,
                "fixed.bin",
                FileMutationOptions::default(),
            )
            .unwrap();
        let racer = store.clone();
        let allocated_path = allocated.path.clone();
        store.set_before_content_commit(move || {
            racer
                .replace_allocated(
                    "cas",
                    &allocated_path,
                    AllocationSource::Bytes(b"allocated racer"),
                    FileMutationOptions::default(),
                )
                .unwrap();
        });
        assert!(matches!(
            store.replace_file_content(
                "cas",
                &allocated.path,
                &allocated.hash,
                AllocationSource::Bytes(b"allocated replacement"),
                FileMutationOptions::default(),
            ),
            Err(StoreError::StaleContentHash(current))
                if current == blake3::hash(b"allocated racer").to_hex().as_str()
        ));
        assert!(
            !store
                .blob_path(blake3::hash(b"allocated replacement").to_hex().as_ref())
                .exists()
        );
    }

    #[test]
    fn file_sources_are_staged_once_before_later_reads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("staged", "data.txt", b"base").unwrap();
        let allocation_source = dir.path().join("allocation-source");
        fs::write(&allocation_source, b"original allocation").unwrap();
        let changed_source = allocation_source.clone();
        store.set_before_content_commit(move || {
            fs::write(changed_source, b"changed allocation").unwrap();
        });
        let allocated = store
            .allocate_file(
                "staged",
                &allocation_source,
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(allocated.size, b"original allocation".len() as u64);
        assert_eq!(
            store.read_blob(&allocated.hash).unwrap().as_ref(),
            b"original allocation"
        );

        let insertion = dir.path().join("splice-source");
        fs::write(&insertion, b"original insertion").unwrap();
        let changed_insertion = insertion.clone();
        store.set_before_content_commit(move || {
            fs::write(changed_insertion, b"changed insertion").unwrap();
        });
        let base = node_hash(&store, "staged", "data.txt");
        let spliced = store
            .splice_file(
                "staged",
                "data.txt",
                &base,
                &[Splice {
                    offset: 4,
                    delete: 0,
                    insert: SpliceSource::File(&insertion),
                }],
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            store.read_blob(&spliced.hash).unwrap().as_ref(),
            b"baseoriginal insertion"
        );
    }

    #[test]
    fn allocated_replacement_reuses_existing_destination_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("reuse", "index.html", b"site").unwrap();
        let first = store
            .allocate_bytes(
                "reuse",
                b"first",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        let destination = store
            .allocate_bytes(
                "reuse",
                b"destination",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();

        let relocated = store
            .replace_allocated(
                "reuse",
                &first.path,
                AllocationSource::Bytes(b"destination"),
                FileMutationOptions::default(),
            )
            .unwrap();

        assert!(relocated.changed);
        assert_eq!(relocated.path, destination.path);
        assert!(matches!(
            store.lookup("reuse", &first.path),
            Err(StoreError::NotFound)
        ));
        assert_eq!(
            node_hash(&store, "reuse", &destination.path),
            destination.hash
        );
        store.undo("reuse", None).unwrap();
        assert_eq!(node_hash(&store, "reuse", &first.path), first.hash);
        assert_eq!(
            node_hash(&store, "reuse", &destination.path),
            destination.hash
        );
    }

    #[test]
    fn pending_finalize_and_regular_noop_replay_after_later_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("replay", "data.txt", b"same").unwrap();
        let base = node_hash(&store, "replay", "data.txt");
        let noop_key = Idempotency {
            key: "regular-noop".to_string(),
        };
        let noop_options = FileMutationOptions {
            idempotency: Some(&noop_key),
            ..FileMutationOptions::default()
        };
        let noop = store
            .replace_file_content(
                "replay",
                "data.txt",
                &base,
                AllocationSource::Bytes(b"same"),
                noop_options,
            )
            .unwrap();
        assert!(!noop.changed);
        store.put_file("replay", "data.txt", b"later").unwrap();
        let replay = store
            .replace_file_content(
                "replay",
                "data.txt",
                &base,
                AllocationSource::Bytes(b"same"),
                noop_options,
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(
            node_hash(&store, "replay", "data.txt"),
            blake3::hash(b"later").to_hex().as_str()
        );

        let pending = store
            .propose_allocation(
                "replay",
                AllocationSource::Bytes(b"pending"),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        let finalize_key = Idempotency {
            key: "pending-finalize".to_string(),
        };
        let finalize_options = FileMutationOptions {
            idempotency: Some(&finalize_key),
            ..FileMutationOptions::default()
        };
        let naming = AllocatedName {
            prefix: "p-",
            suffix: "",
            extension: Some("bin"),
        };
        let finalized = store
            .finalize_allocation("replay", &pending.token, naming, finalize_options)
            .unwrap();
        store.put_file("replay", "later.txt", b"later").unwrap();
        let replayed = store
            .finalize_allocation("replay", &pending.token, naming, finalize_options)
            .unwrap();
        assert!(replayed.replayed);
        assert_eq!(replayed.path, finalized.path);
        assert!(matches!(
            store.finalize_allocation(
                "replay",
                &pending.token,
                AllocatedName {
                    prefix: "different-",
                    ..naming
                },
                finalize_options,
            ),
            Err(StoreError::IdempotencyConflict)
        ));
    }

    #[test]
    fn pending_fingerprint_detects_size_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("tamper", "index.html", b"site").unwrap();
        let pending = store
            .propose_allocation(
                "tamper",
                AllocationSource::Bytes(b"payload"),
                PendingAllocationSpec::default(),
                None,
            )
            .unwrap();
        {
            let mut db = store.inner.writer.lock().unwrap();
            diesel::update(pending_allocations::table.find(&pending.token))
                .set(pending_allocations::size.eq(999_i64))
                .execute(&mut *db)
                .unwrap();
        }
        assert!(matches!(
            store.finalize_allocation(
                "tamper",
                &pending.token,
                AllocatedName::default(),
                FileMutationOptions::default(),
            ),
            Err(StoreError::InvalidPendingAllocation)
        ));
    }

    #[test]
    fn failed_allocation_authorization_never_materializes_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        let missing_hash = blake3::hash(b"missing").to_hex().to_string();
        assert!(matches!(
            store.allocate_bytes(
                "missing",
                b"missing",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            ),
            Err(StoreError::NotFound)
        ));
        assert!(!store.blob_path(&missing_hash).exists());

        store.put_file("managed", "index.html", b"site").unwrap();
        let management = ManagementToken::generate().unwrap();
        {
            let mut db = store.inner.writer.lock().unwrap();
            diesel::update(sites::table.filter(sites::name.eq("managed")))
                .set((
                    sites::management_hash.eq(Some(management.hash().as_bytes().as_slice())),
                    sites::management_status.eq(1_i64),
                ))
                .execute(&mut *db)
                .unwrap();
        }
        let forbidden_hash = blake3::hash(b"forbidden").to_hex().to_string();
        assert!(matches!(
            store.propose_allocation(
                "managed",
                AllocationSource::Bytes(b"forbidden"),
                PendingAllocationSpec::default(),
                None,
            ),
            Err(StoreError::Unauthorized)
        ));
        assert!(!store.blob_path(&forbidden_hash).exists());
    }

    #[test]
    fn copy_and_move_file_counts_include_allocated_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("counts", "index.html", b"site").unwrap();
        store
            .allocate_bytes(
                "counts",
                b"asset",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        let (_, copied) = store
            .copy_site("counts", Some("counts-copy"), None)
            .unwrap();
        assert_eq!(copied.files, 2);
        let (_, moved) = store.move_site("counts-copy", "counts-moved").unwrap();
        assert_eq!(moved.files, 2);
    }

    #[test]
    fn partial_expiry_keeps_site_with_allocated_survivor() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::clone(&clock) as Arc<dyn Clock>,
        )
        .unwrap();
        store
            .put_file("survivor", "temporary.txt", b"temporary")
            .unwrap();
        let survivor = store
            .allocate_bytes(
                "survivor",
                b"allocated survivor",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "survivor",
                "temporary.txt",
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 1,
                }),
            )
            .unwrap();

        clock.advance(1_001);
        assert_eq!(store.sweep_expired().unwrap(), 1);
        assert!(matches!(
            store.lookup("survivor", "temporary.txt"),
            Err(StoreError::NotFound)
        ));
        assert_eq!(node_hash(&store, "survivor", &survivor.path), survivor.hash);
    }

    #[test]
    fn allocated_replace_and_splice_replay_after_source_relocation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("relocation-replay", "index.html", b"site")
            .unwrap();
        let allocated = store
            .allocate_bytes(
                "relocation-replay",
                b"base",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        let replace_key = Idempotency {
            key: "allocated-replace-replay".to_string(),
        };
        let replace_options = FileMutationOptions {
            idempotency: Some(&replace_key),
            ..FileMutationOptions::default()
        };
        let replaced = store
            .replace_file_content(
                "relocation-replay",
                &allocated.path,
                &allocated.hash,
                AllocationSource::Bytes(b"replaced"),
                replace_options,
            )
            .unwrap();
        assert_ne!(replaced.path, allocated.path);
        let replace_replay = store
            .replace_file_content(
                "relocation-replay",
                &allocated.path,
                &allocated.hash,
                AllocationSource::Bytes(b"replaced"),
                replace_options,
            )
            .unwrap();
        assert!(replace_replay.replayed);
        assert_eq!(replace_replay.path, replaced.path);

        let splice_key = Idempotency {
            key: "allocated-splice-replay".to_string(),
        };
        let splice_options = FileMutationOptions {
            idempotency: Some(&splice_key),
            ..FileMutationOptions::default()
        };
        let splice = [Splice {
            offset: 0,
            delete: 0,
            insert: SpliceSource::Bytes(b"x"),
        }];
        let spliced = store
            .splice_file(
                "relocation-replay",
                &replaced.path,
                &replaced.hash,
                &splice,
                splice_options,
            )
            .unwrap();
        assert_ne!(spliced.path, replaced.path);
        let splice_replay = store
            .splice_file(
                "relocation-replay",
                &replaced.path,
                &replaced.hash,
                &splice,
                splice_options,
            )
            .unwrap();
        assert!(splice_replay.replayed);
        assert_eq!(splice_replay.path, spliced.path);
    }

    #[test]
    fn allocation_reuse_and_idempotency_require_metadata_compatibility() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("metadata-reuse", "index.html", b"site")
            .unwrap();
        store
            .allocate_bytes(
                "metadata-reuse",
                b"destination",
                AllocationSpec {
                    media_type: "application/first",
                    ..AllocationSpec::default()
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        assert!(matches!(
            store.allocate_bytes(
                "metadata-reuse",
                b"destination",
                AllocationSpec {
                    media_type: "application/different",
                    ..AllocationSpec::default()
                },
                FileMutationOptions::default(),
            ),
            Err(StoreError::DestinationConflict)
        ));

        let key = Idempotency {
            key: "metadata-fingerprint".to_string(),
        };
        let options = FileMutationOptions {
            idempotency: Some(&key),
            ..FileMutationOptions::default()
        };
        store
            .allocate_bytes(
                "metadata-reuse",
                b"fingerprinted",
                AllocationSpec {
                    media_type: "application/first",
                    ..AllocationSpec::default()
                },
                options,
            )
            .unwrap();
        assert!(matches!(
            store.allocate_bytes(
                "metadata-reuse",
                b"fingerprinted",
                AllocationSpec {
                    media_type: "application/different",
                    ..AllocationSpec::default()
                },
                options,
            ),
            Err(StoreError::IdempotencyConflict)
        ));
    }

    #[test]
    fn destination_reuse_does_not_refresh_destination_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(TestClock::new(1_700_000_000_000));
        let store = Store::with_clock(
            dir.path().to_path_buf(),
            "http://symbol".to_string(),
            Arc::clone(&clock) as Arc<dyn Clock>,
        )
        .unwrap();
        store
            .put_file("metadata-reuse", "index.html", b"site")
            .unwrap();
        let destination = store
            .allocate_bytes(
                "metadata-reuse",
                b"destination",
                AllocationSpec {
                    media_type: "application/first",
                    ..AllocationSpec::default()
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        store
            .set_expiry(
                "metadata-reuse",
                &destination.path,
                Some(ExpiryPolicy::Relative {
                    duration_seconds: 600,
                }),
            )
            .unwrap();
        let refreshed_before = store
            .expiry_report("metadata-reuse", &destination.path)
            .unwrap()
            .refreshed_at;
        let source = store
            .allocate_bytes(
                "metadata-reuse",
                b"source",
                AllocationSpec {
                    media_type: "application/first",
                    ..AllocationSpec::default()
                },
                FileMutationOptions::default(),
            )
            .unwrap();
        clock.advance(10_000);
        store
            .replace_allocated(
                "metadata-reuse",
                &source.path,
                AllocationSource::Bytes(b"destination"),
                FileMutationOptions::default(),
            )
            .unwrap();
        assert_eq!(
            store
                .expiry_report("metadata-reuse", &destination.path)
                .unwrap()
                .refreshed_at,
            refreshed_before
        );
    }

    #[test]
    fn stats_and_site_lists_include_allocated_content_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("allocated-stats", "data.txt", b"same")
            .unwrap();
        store
            .allocate_bytes(
                "allocated-stats",
                b"same",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.sites, 1);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.blobs, 1);
        assert_eq!(stats.logical_bytes, 8);
        assert_eq!(stats.bytes, 4);
        assert_eq!(stats.saved_bytes, 4);
        assert_eq!(stats.file_sizes.min, Some(4));
        assert_eq!(stats.blob_sizes.min, Some(4));

        let sites = store.list_sites().unwrap();
        assert_eq!(sites.files, 2);
        assert_eq!(sites.bytes, 8);
        assert_eq!(sites.entries[0].files, 2);
        assert_eq!(sites.entries[0].bytes, 8);
    }

    #[test]
    fn splice_file_staging_stops_at_configured_limit() {
        let dir = tempfile::tempdir().unwrap();
        let insertion = dir.path().join("oversized");
        fs::write(&insertion, [7_u8; 32]).unwrap();
        let temporary = dir.path().join("staging");
        fs::create_dir(&temporary).unwrap();

        assert!(matches!(
            prepare_splices(
                &[Splice {
                    offset: 0,
                    delete: 0,
                    insert: SpliceSource::File(&insertion),
                }],
                &temporary,
                8,
            ),
            Err(StoreError::SpliceResultTooLarge)
        ));
        assert!(fs::metadata(temporary.join("insert-0")).unwrap().len() <= 9);
    }

    #[test]
    fn splice_missing_and_stale_lookups_remove_staging_directories() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .put_file("splice-cleanup", "data.txt", b"base")
            .unwrap();
        let base = node_hash(&store, "splice-cleanup", "data.txt");
        let insertion = dir.path().join("insertion");
        fs::write(&insertion, b"insert").unwrap();
        let splice = [Splice {
            offset: 0,
            delete: 0,
            insert: SpliceSource::File(&insertion),
        }];
        let splice_directories = || {
            fs::read_dir(dir.path().join("tmp"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("splice-"))
                .count()
        };
        assert_eq!(splice_directories(), 0);

        assert!(matches!(
            store.splice_file(
                "splice-cleanup",
                "missing.txt",
                &base,
                &splice,
                FileMutationOptions::default(),
            ),
            Err(StoreError::NotFound)
        ));
        assert_eq!(splice_directories(), 0);

        assert!(matches!(
            store.splice_file(
                "missing-site",
                "data.txt",
                &base,
                &splice,
                FileMutationOptions::default(),
            ),
            Err(StoreError::NotFound)
        ));
        assert_eq!(splice_directories(), 0);

        assert!(matches!(
            store.splice_file(
                "splice-cleanup",
                "data.txt",
                "stale",
                &splice,
                FileMutationOptions::default(),
            ),
            Err(StoreError::StaleContentHash(current)) if current == base
        ));
        assert_eq!(splice_directories(), 0);
    }

    #[test]
    fn allocated_stale_and_conflicting_writes_do_not_materialize_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("no-orphan", "index.html", b"site").unwrap();
        let allocated = store
            .allocate_bytes(
                "no-orphan",
                b"base",
                AllocationSpec::default(),
                FileMutationOptions::default(),
            )
            .unwrap();
        let stale_bytes = b"stale result";
        let stale_hash = blake3::hash(stale_bytes).to_hex().to_string();
        assert!(matches!(
            store.replace_file_content(
                "no-orphan",
                &allocated.path,
                "wrong-base",
                AllocationSource::Bytes(stale_bytes),
                FileMutationOptions::default(),
            ),
            Err(StoreError::StaleContentHash(_))
        ));
        assert!(!store.blob_path(&stale_hash).exists());

        let conflict_bytes = b"conflicting result";
        let conflict_hash = blake3::hash(conflict_bytes).to_hex().to_string();
        let conflict_path = conflict_hash.clone();
        store
            .put_file("no-orphan", &conflict_path, b"occupied")
            .unwrap();
        assert!(matches!(
            store.replace_allocated(
                "no-orphan",
                &allocated.path,
                AllocationSource::Bytes(conflict_bytes),
                FileMutationOptions::default(),
            ),
            Err(StoreError::DestinationConflict)
        ));
        assert!(!store.blob_path(&conflict_hash).exists());
    }

    #[test]
    fn alias_only_archive_is_an_atomic_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store")).unwrap();
        store
            .put_file("alias-only", "existing.txt", b"target")
            .unwrap();
        let archive = pack_tar(&[ArchiveFile::Alias {
            path: "live.txt".to_string(),
            target: "existing.txt".to_string(),
        }])
        .unwrap();
        let source = dir.path().join("aliases.tar");
        fs::write(&source, archive).unwrap();
        let (name, mutation) = store
            .publish_uploaded_archive(
                Some("alias-only"),
                Some("aliases.tar"),
                &source,
                Kind::Tar,
                PublishOptions::default(),
            )
            .unwrap();
        assert_eq!(name, "alias-only");
        assert!(mutation.changed);
        assert_eq!(
            store
                .alias("alias-only", "live.txt")
                .unwrap()
                .canonical_target,
            "existing.txt"
        );
    }
}
