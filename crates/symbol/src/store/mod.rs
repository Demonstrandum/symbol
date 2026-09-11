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
    ServingStats, SiteEvent, SiteEventKind, SiteInventory, SizeDistribution, Stats, UndoEntry,
    UndoStack,
};

use crate::blob_store::BlobFiles;
use crate::database;
use crate::expiry::{
    DecayPolicy, ExpiryError, ExpiryLimit, ExpiryMode, ExpiryPolicy, ExpiryReport,
    ExpirySiteReport, ExpiryTarget, ExpiryTargetKind, InheritedExpiryCap, OwnExpiryReport,
    remaining_seconds,
};
use crate::hash::{ContentHash, HashParseError, TreeHash};
use crate::name::{NameError, generate_id, parse_site_name};
use crate::pathutil::{PathError, is_junk, is_noise_path, safe_rel_path};
use crate::sanitize::{self, TokenCounts};
use crate::schema::{
    aliases, allocated_entries, blobs, expiry_policies, files, idempotency_records,
    management_audit, management_idempotency, management_tombstones, metadata, path_aggregates,
    pending_allocations, site_entries, site_events, sites, undo_alias_deltas,
    undo_allocated_deltas, undo_expiry_policies, undo_file_deltas, undo_files, undo_names,
    undo_operations, undo_sites,
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

// `store` is split by concern. Each child shares this module's imports and
// types through `use super::*;`, and the glob imports below bring their
// helpers back so the parent and the other children can reach them.
mod types;
#[allow(clippy::wildcard_imports)] // see the note in the child module
pub use types::*;

// `alias`, not `aliases`: the Diesel table module `aliases` already owns that name here.
mod alias;
#[allow(clippy::wildcard_imports)] // see the note in the child module
use alias::*;

mod allocation;
pub use allocation::normalize_allocated_extension;
#[allow(clippy::wildcard_imports)] // see the note in the child module
use allocation::*;

mod expiry;
#[allow(clippy::wildcard_imports)] // see the note in the child module
use expiry::*;

mod mutation;
pub use mutation::validate_mutation_target;
#[allow(clippy::wildcard_imports)] // see the note in the child module
use mutation::*;

mod undo;
#[allow(clippy::wildcard_imports)] // see the note in the child module
use undo::*;

#[cfg(test)]
mod tests;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllocatedMetadata {
    hash: ContentHash,
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
    hash: ContentHash,
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
    hash: ContentHash,
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
    hash: ContentHash,
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
    hash: Option<ContentHash>,
    size: Option<i64>,
    naming_mode: Option<i64>,
    prefix: Option<String>,
    suffix: Option<String>,
    extension: Option<String>,
    media_type: Option<String>,
}

#[derive(Clone, Copy)]
enum PendingFinalName<'a> {
    #[cfg(test)]
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
    entries: HashMap<ContentHash, CachedBlob>,
    recency: BTreeMap<u64, ContentHash>,
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

    fn persist(mut self) -> PathBuf {
        let path = std::mem::take(&mut self.path);
        std::mem::forget(self);
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StagedFile {
    path: String,
    size: i64,
    hash: ContentHash,
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
        hash: ContentHash,
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

    fn get(&self, hash: ContentHash) -> Option<Bytes> {
        let mut state = self.state.lock().unwrap();
        let Some((last_used, bytes)) = state
            .entries
            .get(&hash)
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
            .get_mut(&hash)
            .expect("cached blob still exists")
            .last_used = generation;
        drop(state);
        self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
        Some(bytes)
    }

    fn insert(&self, hash: ContentHash, bytes: Bytes) {
        let charge = bytes
            .len()
            .saturating_add(size_of::<ContentHash>().saturating_mul(2))
            .saturating_add(BLOB_CACHE_ENTRY_OVERHEAD);
        if charge > self.capacity || self.max_entries == 0 {
            return;
        }

        let mut state = self.state.lock().unwrap();
        if let Some(previous) = state.entries.remove(&hash) {
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
        state.recency.insert(generation, hash);
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

    fn remove(&self, hashes: &[ContentHash]) {
        let mut state = self.state.lock().unwrap();
        for hash in hashes {
            if let Some(removed) = state.entries.remove(hash) {
                state.recency.remove(&removed.last_used);
                state.charge -= removed.charge;
            }
        }
    }

    #[cfg(test)]
    fn contains(&self, hash: ContentHash) -> bool {
        self.state.lock().unwrap().entries.contains_key(&hash)
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

    pub fn blob_path(&self, hash: ContentHash) -> PathBuf {
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
            .load::<(ContentHash, i64)>(&mut *db)?;
        referenced_blobs.extend(
            allocated_entries::table
                .select((allocated_entries::hash, allocated_entries::size))
                .load::<(ContentHash, i64)>(&mut *db)?,
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
        let (created, updated) = sites::table
            .find(site_id)
            .select((sites::created, sites::updated))
            .first::<(Option<i64>, i64)>(&mut *snapshot)?;
        let events = site_events::table
            .filter(site_events::site_id.eq(site_id))
            .order((site_events::occurred.desc(), site_events::id.desc()))
            .limit(100)
            .select((site_events::kind, site_events::occurred, site_events::files))
            .load::<(i64, i64, i64)>(&mut *snapshot)?
            .into_iter()
            .map(|(kind, occurred, files)| SiteEvent {
                kind: site_event_kind(kind),
                at: format_timestamp(occurred),
                files: files.max(0).cast_unsigned(),
            })
            .collect();
        let rows = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .order(files::path)
            .load::<(String, ContentHash, i64)>(&mut *snapshot)?;
        let mut inventory = rows
            .into_iter()
            .map(|(path, hash, size)| InventoryFile {
                path,
                hash: format!("blake3:{}", hash.to_hex()),
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
            .load::<(String, ContentHash, i64)>(&mut *snapshot)?;
        for (path, hash, size) in allocated {
            inventory.push(InventoryFile {
                hash: format!("blake3:{}", hash.to_hex()),
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
                    resolved_hash: alias.resolved_hash,
                    size: alias.resolved_size,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        snapshot.commit()?;
        Ok(SiteInventory {
            site: name.to_string(),
            created_at: created.map(format_timestamp),
            updated_at: format_timestamp(updated),
            content_revision: revision,
            tree_hash: tree_hash.to_wire(),
            events,
            files: inventory,
            aliases,
        })
    }

    pub fn site_updated_at(&self, name: &str) -> Option<i64> {
        let Ok(name) = parse_site_name(name) else {
            return None;
        };
        let mut db = self.inner.readers.get();
        sites::table
            .filter(sites::name.eq(name))
            .select(sites::updated)
            .first::<i64>(&mut *db)
            .ok()
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

    pub fn read_blob(&self, hash: ContentHash) -> Result<Bytes, StoreError> {
        if let Some(bytes) = self.inner.blobs.get(hash) {
            return Ok(bytes);
        }
        let mut db = self.inner.readers.get();
        blobs::table
            .find(hash)
            .select(blobs::hash)
            .first::<ContentHash>(&mut *db)
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

    pub fn site_references_blob(&self, name: &str, hash: ContentHash) -> Result<bool, StoreError> {
        let name = parse_site_name(name)?;
        let content_hash = hash;
        let mut db = self.inner.readers.get();
        let count = files::table
            .inner_join(sites::table)
            .filter(sites::name.eq(name))
            .filter(files::hash.eq(content_hash))
            .select(count_star())
            .first::<i64>(&mut *db)?;
        if count != 0 {
            return Ok(true);
        }
        let site_id = site_id_locked(&mut db, name)?;
        let count = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::hash.eq(content_hash))
            .select(count_star())
            .first::<i64>(&mut *db)?;
        Ok(count != 0)
    }

    fn remove_blob_files(&self, hashes: &[ContentHash]) {
        self.inner.blobs.remove(hashes);
        for &hash in hashes {
            if let Err(err) = self.inner.blob_files.quarantine(hash) {
                tracing::warn!(%hash, %err, "failed to quarantine unreferenced blob file");
            }
        }
    }

    fn restore_quarantined_blob_files(&self) -> Result<(), StoreError> {
        let mut db = self.inner.readers.get();
        let live = blobs::table
            .select(blobs::hash)
            .load::<ContentHash>(&mut *db)?
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
                .load::<(ContentHash, Vec<u8>, i64)>(&mut *db)?;
            for (hash, bytes, size) in rows {
                if i64::try_from(bytes.len()).expect("blob size fits in i64") != size
                    || ContentHash::from(blake3::hash(&bytes)) != hash
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("corrupt SQLite blob {hash}"),
                    )
                    .into());
                }
                self.inner.blob_files.put_bytes(hash, &bytes)?;
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
    #[expect(clippy::missing_const_for_fn, clippy::unused_self)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
enum StoredSiteEventKind {
    Created = 1,
    Publish = 2,
    Rename = 3,
    Restore = 4,
}

impl StoredSiteEventKind {
    const fn from_code(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Created),
            2 => Some(Self::Publish),
            3 => Some(Self::Rename),
            4 => Some(Self::Restore),
            _ => None,
        }
    }

    const fn contract(self) -> SiteEventKind {
        match self {
            Self::Created => SiteEventKind::Created,
            Self::Publish => SiteEventKind::Publish,
            Self::Rename => SiteEventKind::Rename,
            Self::Restore => SiteEventKind::Restore,
        }
    }
}

fn record_site_event(
    tx: &mut SqliteConnection,
    site_id: i64,
    kind: StoredSiteEventKind,
    files: usize,
    now: i64,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(site_events::table)
        .values((
            site_events::site_id.eq(site_id),
            site_events::kind.eq(kind as i64),
            site_events::occurred.eq(now),
            site_events::files.eq(i64::try_from(files).unwrap_or(i64::MAX)),
        ))
        .execute(tx)?;
    Ok(())
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
    replace: bool,
}

#[derive(Clone, Copy)]
struct ArchiveAlias<'a> {
    path: &'a str,
    target: &'a str,
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

fn authorization_fingerprint(token: &ManagementToken) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in token.hash().as_bytes() {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
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

fn site_revision_locked(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<(u64, TreeHash), StoreError> {
    let (revision, tree_hash) = sites::table
        .filter(sites::name.eq(name))
        .select((sites::content_revision, sites::tree_hash))
        .first::<(i64, TreeHash)>(db)
        .map_err(map_sql)?;
    Ok((revision.cast_unsigned(), tree_hash))
}

fn site_event_kind(kind: i64) -> SiteEventKind {
    let stored = StoredSiteEventKind::from_code(kind)
        .unwrap_or_else(|| panic!("stored site event kind {kind} is unknown"));
    stored.contract()
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
    File { hash: ContentHash },
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
        .first::<ContentHash>(db)
        .optional()?;
    if let Some(hash) = hash {
        return Ok(NodeKind::File { hash });
    }
    let allocated = allocated_entries::table
        .find((site_id, rel))
        .select(allocated_entries::hash)
        .first::<ContentHash>(db)
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
                let bytes = blobs.read(hash)?;
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
