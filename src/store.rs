use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::blob_store::BlobFiles;
use crate::expiry::{
    DecayPolicy, ExpiryError, ExpiryLimit, ExpiryMode, ExpiryPolicy, ExpiryReport, ExpiryTarget,
    ExpiryTargetKind, InheritedExpiryCap, OwnExpiryReport, remaining_seconds,
};
use crate::name::{NameError, generate_id, parse_site_name};
use crate::pathutil::{PathError, is_junk, is_noise_path, looks_like_apple_fork, safe_rel_path};
use crate::sanitize::{self, TokenCounts};
use crate::secrets::{ClaimToken, ClaimTokenHash, ManagementToken, ManagementTokenHash};
#[cfg(test)]
use crate::upload::write_payload;
use crate::upload::{Kind, UploadError, write_payload_file};

#[cfg(test)]
use std::io::Cursor;

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS sites (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
    hash TEXT PRIMARY KEY,
    bytes BLOB NOT NULL DEFAULT X'',
    size INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    hash TEXT NOT NULL REFERENCES blobs(hash),
    size INTEGER NOT NULL,
    PRIMARY KEY (site_id, path)
);
CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS files_site_prefix ON files(site_id, path);
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";
const LATEST_SCHEMA_VERSION: i64 = 4;
const UNDO_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;
const UNDO_LIMIT_PER_SITE: i64 = 10;
const IDEMPOTENCY_RETENTION_MILLIS: i64 = 4 * 60 * 60 * 1000;
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

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    writer: Mutex<Connection>,
    readers: ReaderPool,
    blobs: BlobCache,
    blob_files: BlobFiles,
    metrics: Arc<Metrics>,
    public_url: String,
    clock: Arc<dyn Clock>,
    expiry_defaults: DecayPolicy,
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
    available: Mutex<Vec<Connection>>,
    ready: Condvar,
    size: usize,
    metrics: Arc<Metrics>,
}

struct Reader<'a> {
    pool: &'a ReaderPool,
    connection: Option<Connection>,
    acquired: Instant,
}

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

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ServingStats {
    pub cache: CacheStats,
    pub readers: ReaderStats,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ReaderStats {
    pub operations: u64,
    pub waits: u64,
    pub wait_micros: u64,
    pub query_micros: u64,
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
    pub bytes: u64,
    pub entries: Vec<SiteEnt>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SizeDistribution {
    pub min: Option<u64>,
    pub p25: Option<f64>,
    pub median: Option<f64>,
    pub mean: Option<f64>,
    pub p75: Option<f64>,
    pub max: Option<u64>,
    pub iqr: Option<f64>,
    pub stddev: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Stats {
    pub sites: u64,
    pub files: u64,
    pub blobs: u64,
    pub bytes: u64,
    pub logical_bytes: u64,
    pub saved_bytes: u64,
    pub saved_fraction: f64,
    pub file_sizes: SizeDistribution,
    pub blob_sizes: SizeDistribution,
    pub serving: ServingStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Clone, Copy, Default)]
pub struct PublishOptions<'a> {
    pub expected_tree_hash: Option<&'a str>,
    pub idempotency: Option<&'a Idempotency>,
    pub creation: CreationSecurity,
    pub authorization: Option<&'a ManagementToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum CreatorKind {
    TrustedProxy = 1,
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
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CreationSecurity {
    pub creator: Option<CreatorIdentity>,
    pub claim_hash: Option<ClaimTokenHash>,
    pub management_hash: Option<ManagementTokenHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ManagementStatus {
    pub managed: bool,
}

#[derive(Debug)]
pub struct ManagementMutation {
    pub status: ManagementStatus,
    pub token: Option<ManagementToken>,
    pub replayed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InventoryFile {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteInventory {
    pub site: String,
    pub content_revision: u64,
    pub tree_hash: String,
    pub files: Vec<InventoryFile>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoEntry {
    pub token: String,
    pub kind: String,
    pub description: String,
    pub created_at: String,
    pub expires_at: String,
    pub remaining_seconds: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoStack {
    pub site: String,
    pub entries: Vec<UndoEntry>,
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
}

#[cfg(test)]
struct ArchiveFile {
    path: String,
    bytes: Vec<u8>,
}

struct ArchiveEntry {
    path: String,
    hash: String,
    size: u64,
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
    #[error("error: destination site already exists")]
    DestinationConflict,
    #[error("error: idempotency key was already used for a different request")]
    IdempotencyConflict,
    #[error("error: idempotency key must be 1-256 visible ASCII characters")]
    InvalidIdempotencyKey,
    #[error("error: upstream changed; nothing was written")]
    PreconditionFailed { revision: u64, tree_hash: String },
    #[error("error: reserved path already exists: {0}")]
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
    Sqlite(#[from] rusqlite::Error),
    #[error("error: operating system random source failed")]
    Random(#[from] getrandom::Error),
    #[error("error: {0}")]
    Io(#[from] io::Error),
}

impl ReaderPool {
    fn open(path: &Path, count: usize, metrics: Arc<Metrics>) -> Result<Self, rusqlite::Error> {
        let mut available = Vec::with_capacity(count);
        for _ in 0..count {
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            connection.busy_timeout(std::time::Duration::from_secs(5))?;
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
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().unwrap()
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
        let mut db = Connection::open(&path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
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
                public_url,
                clock,
                expiry_defaults,
            }),
        };
        store.migrate_sqlite_blobs()?;
        store.migrate_legacy()?;
        store.gc_junk()?;
        store.backfill_manifests()?;
        store.sweep_expired()?;
        store.prune_undo_and_gc()?;
        store.gc_blob_files()?;
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
        let db = self.inner.readers.get();
        let sites = db.query_row("SELECT COUNT(*) FROM sites", [], |row| {
            row.get::<_, i64>(0).map(i64::cast_unsigned)
        })?;
        let file_values = load_sizes(
            &db,
            "SELECT size FROM files WHERE path <> 'symbol.toml' ORDER BY size",
        )?;
        let blob_values = load_sizes(
            &db,
            "SELECT blobs.size
             FROM blobs JOIN files ON files.hash = blobs.hash
             WHERE files.path <> 'symbol.toml'
             GROUP BY blobs.hash, blobs.size
             ORDER BY blobs.size",
        )?;
        let files = u64::try_from(file_values.len()).expect("file count fits in u64");
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
        let db = self.inner.readers.get();
        let mut stmt = db.prepare(
            "SELECT sites.name, COUNT(files.path), COALESCE(SUM(files.size), 0)
             FROM sites LEFT JOIN files ON files.site_id = sites.id
             GROUP BY sites.id, sites.name
             ORDER BY sites.name",
        )?;
        let entries = stmt
            .query_map([], |row| {
                Ok(SiteEnt {
                    name: row.get(0)?,
                    files: row.get::<_, i64>(1)?.cast_unsigned(),
                    bytes: row.get::<_, i64>(2)?.cast_unsigned(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let files = entries.iter().map(|entry| entry.files).sum();
        let bytes = entries.iter().map(|entry| entry.bytes).sum();
        Ok(SiteList {
            files,
            bytes,
            entries,
        })
    }

    #[cfg(test)]
    pub fn list_files(&self, name: &str) -> Result<Vec<String>, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        if !site_exists_locked(&db, name)? {
            return Err(StoreError::NotFound);
        }
        let mut stmt = db.prepare(
            "SELECT path FROM files WHERE site_id = (SELECT id FROM sites WHERE name = ?1) ORDER BY path",
        )?;
        let paths = stmt
            .query_map(params![name], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        drop(stmt);
        Ok(paths)
    }

    pub fn list_dir(&self, name: &str, rel: &str) -> Result<DirList, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        if !rel.is_empty() && is_noise_path(Path::new(&rel)) {
            return Err(StoreError::NotFound);
        }
        let db = self.inner.readers.get();
        match node_locked(&db, name, &rel)? {
            NodeKind::Dir => {}
            NodeKind::File { .. } | NodeKind::Missing => return Err(StoreError::NotFound),
        }
        let files = if rel.is_empty() {
            load_root_files(&db, name)?
        } else {
            load_descendant_files(&db, name, &rel)?
        };
        Ok(dirents(&files, &rel))
    }

    pub fn site_inventory(&self, name: &str) -> Result<SiteInventory, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        let (revision, tree_hash) = site_revision_locked(&db, name)?;
        let mut stmt = db.prepare(
            "SELECT path, hash, size FROM files
             WHERE site_id = (SELECT id FROM sites WHERE name = ?1) AND path <> ?2
             ORDER BY path",
        )?;
        let files = stmt
            .query_map(params![name, MANIFEST_PATH], |row| {
                Ok(InventoryFile {
                    path: row.get(0)?,
                    hash: format!("blake3:{}", row.get::<_, String>(1)?),
                    size: row.get::<_, i64>(2)?.cast_unsigned(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SiteInventory {
            site: name.to_string(),
            content_revision: revision,
            tree_hash,
            files,
        })
    }

    pub fn site_exists(&self, name: &str) -> bool {
        let Ok(name) = parse_site_name(name) else {
            return false;
        };
        let db = self.inner.readers.get();
        site_exists_locked(&db, name).unwrap_or(false)
    }

    pub fn authorize_mutation(
        &self,
        name: &str,
        token: Option<&ManagementToken>,
    ) -> Result<(), StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        authorize_locked(&db, name, token)
    }

    pub fn management_status(&self, name: &str) -> Result<ManagementStatus, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        db.query_row(
            "SELECT management_status FROM sites WHERE name = ?1",
            [name],
            |row| {
                Ok(ManagementStatus {
                    managed: row.get(0)?,
                })
            },
        )
        .map_err(map_sql)
    }

    pub fn claim_management(
        &self,
        name: &str,
        creator: Option<CreatorIdentity>,
        claim: Option<&ClaimToken>,
        idempotency: Option<&Idempotency>,
    ) -> Result<ManagementMutation, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        prune_management_idempotency(&tx, now)?;
        let fingerprint = format!("claim:{name}");
        let (site_id, managed, creator_kind, creator_hash, claim_hash) = tx
            .query_row(
                "SELECT id, management_status, creator_kind, creator_hash, claim_hash
                 FROM sites WHERE name = ?1",
                [name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                    ))
                },
            )
            .map_err(map_sql)?;
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
        if management_replay(&tx, idempotency, &fingerprint)? {
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
        tx.execute(
            "UPDATE sites SET management_hash = ?1, management_status = 1 WHERE id = ?2",
            params![token.hash().as_bytes().as_slice(), site_id],
        )?;
        record_management(&tx, name, 1, now)?;
        store_management_idempotency(&tx, idempotency, &fingerprint, now)?;
        regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
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
        idempotency: Option<&Idempotency>,
    ) -> Result<ManagementMutation, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        prune_management_idempotency(&tx, now)?;
        let fingerprint = format!("rotate:{name}");
        let (site_id, managed, expected_hash, creator_kind, creator_hash, claim_hash) = tx
            .query_row(
                "SELECT id, management_status, management_hash,
                        creator_kind, creator_hash, claim_hash
                 FROM sites WHERE name = ?1",
                [name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                },
            )
            .map_err(map_sql)?;
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
        if management_replay(&tx, idempotency, &fingerprint)? {
            return Ok(ManagementMutation {
                status: ManagementStatus { managed: true },
                token: None,
                replayed: true,
            });
        }
        let token = ManagementToken::generate()?;
        tx.execute(
            "UPDATE sites SET management_hash = ?1 WHERE id = ?2",
            params![token.hash().as_bytes().as_slice(), site_id],
        )?;
        record_management(&tx, name, 2, now)?;
        store_management_idempotency(&tx, idempotency, &fingerprint, now)?;
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
    ) -> Result<ManagementStatus, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        authorize_locked(&tx, name, bearer)?;
        let site_id: i64 = tx
            .query_row("SELECT id FROM sites WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .map_err(map_sql)?;
        tx.execute(
            "UPDATE sites SET management_hash = NULL, management_status = 0 WHERE id = ?1",
            [site_id],
        )?;
        tx.execute("DELETE FROM management_tombstones WHERE name = ?1", [name])?;
        record_management(&tx, name, 3, now)?;
        regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
        tx.commit()?;
        drop(db);
        Ok(ManagementStatus { managed: false })
    }

    pub fn operator_claim(&self, name: &str) -> Result<ManagementToken, StoreError> {
        let name = parse_site_name(name)?;
        let token = ManagementToken::generate()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let (site_id, managed): (i64, bool) = tx
            .query_row(
                "SELECT id, management_status FROM sites WHERE name = ?1",
                [name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sql)?;
        if managed {
            return Err(StoreError::AlreadyManaged);
        }
        tx.execute(
            "UPDATE sites SET management_hash = ?1, management_status = 1 WHERE id = ?2",
            params![token.hash().as_bytes().as_slice(), site_id],
        )?;
        record_management(&tx, name, 4, now)?;
        regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
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
        let tx = db.transaction()?;
        authorize_locked(&tx, name, Some(current))?;
        let site_id: i64 = tx
            .query_row("SELECT id FROM sites WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .map_err(map_sql)?;
        tx.execute(
            "UPDATE sites SET management_hash = ?1 WHERE id = ?2",
            params![token.hash().as_bytes().as_slice(), site_id],
        )?;
        record_management(&tx, name, 5, now)?;
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
        let db = self.inner.readers.get();
        let node = match node_locked(&db, name, &rel)? {
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
        let db = self.inner.readers.get();
        db.query_row("SELECT 1 FROM blobs WHERE hash = ?1", [hash], |_| Ok(()))
            .map_err(map_sql)?;
        drop(db);
        let bytes = Bytes::from(self.inner.blob_files.read(hash)?);
        self.inner.blobs.insert(hash, bytes.clone());
        Ok(bytes)
    }

    pub fn site_references_blob(&self, name: &str, hash: &str) -> Result<bool, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        db.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM files JOIN sites ON sites.id = files.site_id
                WHERE sites.name = ?1 AND files.hash = ?2
            )",
            params![name, hash],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)
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
            write_payload_file(&tmp, source, kind, filename, true)?;
            let staged = stage_dir(&tmp)?;
            if staged.is_empty() {
                return Err(StoreError::Upload(UploadError::EmptyArchive));
            }
            self.publish_staged(wanted, &staged, options)
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
            UndoKind::Put,
            options.expected_tree_hash,
            options.creation,
            options.authorization,
        )
    }

    #[cfg(test)]
    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let archive = site_files(&tx, &self.inner.blob_files, name)?;
        let packed = pack_tar_gz(&archive.files)?;
        snapshot_site(&tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&tx, name, self.now_millis())?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        let removed = gc_blobs(&tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(packed)
    }

    #[cfg(test)]
    pub fn pack_site(&self, name: &str, format: ArchiveFormat) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.readers.get();
        let archive = site_files(&db, &self.inner.blob_files, name)?;
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
        let db = self.inner.readers.get();
        let entries = site_manifest(&db, name)?;
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
        let entries = site_manifest(&db, name)?;
        write_site_archive(&self.inner.blob_files, &entries, format, output)?;
        let tx = db.transaction()?;
        authorize_locked(&tx, name, authorization)?;
        let undo = snapshot_site(&tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&tx, name, self.now_millis())?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        prune_undo_locked(&tx, self.now_millis())?;
        let removed = gc_blobs(&tx, self.now_millis())?;
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
        let tx = db.transaction()?;
        prune_idempotency_locked(&tx, now)?;
        if !site_exists_locked(&tx, source)? {
            return Err(StoreError::NotFound);
        }
        if destination.is_none()
            && let Some(idempotency) = idempotency
        {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let generated = destination.is_none();
        let destination = destination.unwrap_or_else(|| {
            generate_id(|candidate| site_exists_locked(&tx, candidate).unwrap_or(true))
        });
        if site_exists_locked(&tx, &destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let (source_id, public_url, revision): (i64, String, i64) = tx.query_row(
            "SELECT id, public_url, content_revision FROM sites WHERE name = ?1",
            [source],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let undo = snapshot_site_with_description(
            &tx,
            &destination,
            UndoKind::Copy,
            &format!("remove copied site {destination}"),
            now,
        )?;
        tx.execute(
            "INSERT INTO sites(
                name, updated, public_url, content_revision, tree_hash,
                creator_kind, creator_hash, claim_hash, management_hash, management_status
             ) VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9)",
            params![
                destination,
                now,
                public_url,
                revision,
                creation.creator.map(|creator| creator.kind as i64),
                creation.creator.map(|creator| creator.hash.to_vec()),
                creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                i64::from(creation.management_hash.is_some())
            ],
        )?;
        let destination_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO files(site_id, path, hash, size)
             SELECT ?1, path, hash, size FROM files
             WHERE site_id = (SELECT id FROM sites WHERE name = ?2) AND path <> ?3",
            params![destination_id, source, MANIFEST_PATH],
        )?;
        copy_expiry_policies_locked(&tx, source_id, destination_id, now)?;
        let files = tx.query_row(
            "SELECT COUNT(*) FROM files WHERE site_id = ?1",
            [destination_id],
            |row| row.get::<_, i64>(0),
        )?;
        let tree_hash = regenerate_site(&tx, &self.inner.blob_files, destination_id, now)?;
        prune_undo_locked(&tx, now)?;
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
                &tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&tx, now)?;
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
        let tx = db.transaction()?;
        authorize_locked(&tx, source, authorization)?;
        if !site_exists_locked(&tx, source)? {
            return Err(StoreError::NotFound);
        }
        if site_exists_locked(&tx, destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let undo = snapshot_site_with_description(
            &tx,
            source,
            UndoKind::Move,
            &format!("move {destination} back to {source}"),
            now,
        )?;
        tx.execute(
            "INSERT INTO undo_names(token, name) VALUES (?1, ?2)",
            params![undo.token, destination],
        )?;
        tx.execute(
            "UPDATE sites SET name = ?1, updated = ?2 WHERE name = ?3",
            params![destination, now, source],
        )?;
        let (site_id, revision): (i64, i64) = tx.query_row(
            "SELECT id, content_revision FROM sites WHERE name = ?1",
            [destination],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let files = tx.query_row(
            "SELECT COUNT(*) FROM files WHERE site_id = ?1 AND path <> ?2",
            params![site_id, MANIFEST_PATH],
            |row| row.get::<_, i64>(0),
        )?;
        let tree_hash = regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&tx, now)?;
        let removed = gc_blobs(&tx, now)?;
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
        let tx = db.transaction()?;
        authorize_locked(&tx, name, authorization)?;
        reject_reserved_path(&rel)?;
        let site_id: i64 = tx
            .query_row(
                "SELECT id FROM sites WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        let undo = snapshot_site_with_description(
            &tx,
            name,
            UndoKind::DeletePath,
            &format!("restore deleted {rel}"),
            self.now_millis(),
        )?;
        let deleted = tx.execute(
            "DELETE FROM files
             WHERE site_id = ?1
               AND (path = ?2 OR (path >= ?3 AND path < ?4))",
            params![site_id, rel, prefix_start, prefix_end],
        )?;
        if deleted == 0 {
            return Err(StoreError::NotFound);
        }
        tx.execute(
            "DELETE FROM expiry_policies
             WHERE site_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
            params![site_id, rel, prefix_start, prefix_end],
        )?;
        tx.execute(
            "UPDATE sites SET content_revision = content_revision + 1 WHERE id = ?1",
            [site_id],
        )?;
        refresh_expiry_for_changes_locked(&tx, site_id, &[&rel], self.now_millis())?;
        regenerate_site(&tx, &self.inner.blob_files, site_id, self.now_millis())?;
        prune_undo_locked(&tx, self.now_millis())?;
        let removed = gc_blobs(&tx, self.now_millis())?;
        let (revision, tree_hash) = site_revision_locked(&tx, name).unwrap_or((0, String::new()));
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
        let wanted = wanted.filter(|name| !name.is_empty());
        if let Some(name) = wanted {
            let name = parse_site_name(name)?.to_string();
            let mutation = self.merge_staged_conditional(
                &name,
                files,
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
        if files.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let fingerprint = staged_fingerprint(&files);
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        prune_idempotency_locked(&tx, now)?;
        if let Some(idempotency) = options.idempotency {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let name = generate_id(|candidate| site_exists_locked(&tx, candidate).unwrap_or(true));
        let mutation = self.merge_staged_locked(
            &tx,
            &files,
            MergeContext {
                name: &name,
                kind: UndoKind::Put,
                expected_tree_hash: None,
                now,
                creation: options.creation,
                authorization: None,
            },
        )?;
        let published = PublishedMutation {
            name: name.clone(),
            mutation: mutation.clone(),
        };
        if let Some(idempotency) = options.idempotency {
            store_idempotency(
                &tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&tx, now)?;
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
        let files = files
            .iter()
            .filter(|file| file.path != MANIFEST_PATH)
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let mutation = self.merge_staged_locked(
            &tx,
            &files,
            MergeContext {
                name,
                kind,
                expected_tree_hash,
                now,
                creation,
                authorization,
            },
        )?;
        let removed = gc_blobs(&tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(mutation)
    }

    #[allow(clippy::large_types_passed_by_value, clippy::too_many_lines)]
    fn merge_staged_locked(
        &self,
        tx: &rusqlite::Transaction<'_>,
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
        let changed = files.iter().try_fold(false, |changed, file| {
            let current = tx
                .query_row(
                    "SELECT hash FROM files
                     WHERE site_id = (SELECT id FROM sites WHERE name = ?1) AND path = ?2",
                    params![name, file.path],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Ok::<_, rusqlite::Error>(changed || current.as_deref() != Some(file.hash.as_str()))
        })?;
        if !changed {
            let (revision, tree_hash) = site_revision_locked(tx, name)?;
            return Ok(MutationResult {
                created: false,
                changed: false,
                replayed: false,
                files: files.len(),
                revision,
                tree_hash,
                undo: None,
                sanitized: sanitized_counts(files),
            });
        }
        for file in files {
            self.materialize(file)?;
        }
        let description = if existed {
            format!("restore previous state of {name}")
        } else {
            format!("remove newly created site {name}")
        };
        let undo = snapshot_site_with_description(tx, name, kind, &description, now)?;
        for file in files {
            tx.execute(
                "INSERT OR IGNORE INTO blobs (hash, bytes, size) VALUES (?1, X'', ?2)",
                params![file.hash, file.size],
            )?;
        }
        tx.execute(
            "INSERT INTO sites
                (name, updated, public_url, content_revision, tree_hash,
                 creator_kind, creator_hash, claim_hash, management_hash, management_status)
             VALUES (?1, ?2, ?3, 0, '', ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(name) DO NOTHING",
            params![
                name,
                now,
                self.inner.public_url,
                creation.creator.map(|creator| creator.kind as i64),
                creation.creator.map(|creator| creator.hash.to_vec()),
                creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                i64::from(creation.management_hash.is_some())
            ],
        )?;
        let site_id: i64 = tx.query_row(
            "SELECT id FROM sites WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        for file in files {
            tx.execute(
                "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(site_id, path) DO UPDATE SET hash = excluded.hash, size = excluded.size",
                params![site_id, file.path, file.hash, file.size],
            )?;
        }
        let revision = if existed {
            tx.query_row(
                "SELECT content_revision + 1 FROM sites WHERE id = ?1",
                [site_id],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            1
        };
        tx.execute(
            "UPDATE sites SET updated = ?1, content_revision = ?2 WHERE id = ?3",
            params![now, revision, site_id],
        )?;
        let changed_paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        refresh_expiry_for_changes_locked(tx, site_id, &changed_paths, now)?;
        let tree_hash = regenerate_site(tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(tx, now)?;
        Ok(MutationResult {
            created: !existed,
            changed: true,
            replayed: false,
            files: files.len(),
            revision: revision.cast_unsigned(),
            tree_hash,
            undo: Some(undo),
            sanitized: sanitized_counts(files),
        })
    }

    fn materialize(&self, file: &StagedFile) -> Result<(), StoreError> {
        match &file.source {
            StagedSource::Bytes(bytes) => self.inner.blob_files.put_bytes(&file.hash, bytes)?,
            StagedSource::File(path) => self.inner.blob_files.put_file(&file.hash, path)?,
        }
        Ok(())
    }

    fn remove_blob_files(&self, hashes: &[String]) {
        self.inner.blobs.remove(hashes);
        for hash in hashes {
            if let Err(err) = self.inner.blob_files.remove(hash) {
                tracing::warn!(%hash, %err, "failed to remove unreferenced blob file");
            }
        }
    }

    fn migrate_sqlite_blobs(&self) -> Result<(), StoreError> {
        let mut db = self.inner.writer.lock().unwrap();
        let migrated = db
            .query_row(
                "SELECT 1 FROM metadata WHERE key = 'external_blobs_v1'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if migrated {
            return Ok(());
        }

        {
            let mut stmt = db.prepare("SELECT hash, bytes, size FROM blobs ORDER BY hash")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (hash, bytes, size) = row?;
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

        let tx = db.transaction()?;
        tx.execute("UPDATE blobs SET bytes = X''", [])?;
        tx.execute(
            "INSERT INTO metadata (key, value) VALUES ('external_blobs_v1', '1')",
            [],
        )?;
        tx.commit()?;
        if let Err(err) = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;") {
            tracing::warn!(%err, "blob migration succeeded but database compaction failed");
        }
        drop(db);
        Ok(())
    }

    fn migrate_legacy(&self) -> Result<(), StoreError> {
        {
            let db = self.inner.writer.lock().unwrap();
            let n: i64 = db.query_row("SELECT COUNT(*) FROM sites", [], |row| row.get(0))?;
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

    fn gc_junk(&self) -> Result<(), StoreError> {
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let mut apple = HashSet::new();
        {
            let mut stmt = tx.prepare("SELECT hash FROM blobs WHERE size <= 65536")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                let hash = row?;
                let mut prefix = [0_u8; 4];
                let read = fs::File::open(self.inner.blob_files.path(&hash))?.read(&mut prefix)?;
                if looks_like_apple_fork(&prefix[..read]) {
                    apple.insert(hash);
                }
            }
        }
        let mut junk = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT site_id, path, hash FROM files")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (site_id, path, hash) = row?;
                if is_junk(Path::new(&path), None) || apple.contains(&hash) {
                    junk.push((site_id, path));
                }
            }
        }
        let mut sites = HashSet::new();
        for (site_id, path) in &junk {
            tx.execute(
                "DELETE FROM files WHERE site_id = ?1 AND path = ?2",
                params![site_id, path],
            )?;
            sites.insert(*site_id);
        }
        for site_id in sites {
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM files WHERE site_id = ?1",
                params![site_id],
                |row| row.get(0),
            )?;
            if remaining == 0 {
                tx.execute("DELETE FROM sites WHERE id = ?1", params![site_id])?;
            }
        }
        let removed = gc_blobs(&tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }

    fn gc_blob_files(&self) -> Result<(), StoreError> {
        let db = self.inner.readers.get();
        let mut stmt = db.prepare("SELECT hash FROM blobs")?;
        let live = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<HashSet<String>, _>>()?;
        drop(stmt);
        drop(db);
        self.inner.blob_files.retain(&live)?;
        Ok(())
    }

    pub fn undo_stack(&self, name: &str) -> Result<UndoStack, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let db = self.inner.readers.get();
        let mut stmt = db.prepare(
            "SELECT operation.token, operation.kind, operation.description,
                    operation.created, operation.expires
             FROM undo_operations AS operation
             JOIN undo_names AS names ON names.token = operation.token
             WHERE names.name = ?1 AND operation.consumed = 0 AND operation.expires > ?2
             ORDER BY operation.created DESC, operation.rowid DESC",
        )?;
        let entries = stmt
            .query_map(params![name, now], |row| {
                let created = row.get::<_, i64>(3)?;
                let expires = row.get::<_, i64>(4)?;
                Ok(UndoEntry {
                    token: row.get(0)?,
                    kind: UndoKind::from_i64(row.get(1)?).as_str().to_string(),
                    description: row.get(2)?,
                    created_at: format_timestamp(created),
                    expires_at: format_timestamp(expires),
                    remaining_seconds: u64::try_from((expires - now).max(0) / 1000)
                        .expect("remaining time is non-negative"),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
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
        let tx = db.transaction()?;
        authorize_locked(&tx, name, authorization)?;
        let latest = tx
            .query_row(
                "SELECT operation.token
                 FROM undo_operations AS operation
                 JOIN undo_names AS names ON names.token = operation.token
                 WHERE names.name = ?1 AND operation.consumed = 0 AND operation.expires > ?2
                 ORDER BY operation.created DESC, operation.rowid DESC
                 LIMIT 1",
                params![name, now],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        if guard.is_some_and(|token| token != latest) {
            return Err(StoreError::StaleUndo(latest));
        }
        let snapshot = tx.query_row(
            "SELECT name, existed, public_url, updated, content_revision, tree_hash
             FROM undo_sites WHERE token = ?1",
            [&latest],
            |row| {
                Ok(SiteSnapshot {
                    name: row.get(0)?,
                    existed: row.get(1)?,
                    public_url: row.get(2)?,
                    updated: row.get(3)?,
                    content_revision: row.get(4)?,
                    tree_hash: row.get(5)?,
                })
            },
        )?;
        {
            let mut stmt = tx.prepare("SELECT name FROM undo_names WHERE token = ?1")?;
            let names = stmt
                .query_map([&latest], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for name in names {
                retain_management_tombstone(&tx, &name, now)?;
                tx.execute("DELETE FROM sites WHERE name = ?1", [name])?;
            }
        }
        if snapshot.existed {
            tx.execute(
                "INSERT INTO sites
                    (name, updated, public_url, content_revision, tree_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    snapshot.name,
                    snapshot.updated,
                    snapshot.public_url,
                    snapshot.content_revision,
                    snapshot.tree_hash
                ],
            )?;
            let site_id = tx.last_insert_rowid();
            let retained_hash = tx
                .query_row(
                    "SELECT management_hash FROM management_tombstones
                     WHERE name IN (SELECT name FROM undo_names WHERE token = ?1)
                     LIMIT 1",
                    [&latest],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?;
            if let Some(hash) = retained_hash {
                tx.execute(
                    "UPDATE sites
                     SET management_hash = ?1, management_status = 1
                     WHERE id = ?2",
                    params![hash, site_id],
                )?;
            }
            tx.execute(
                "INSERT INTO files (site_id, path, hash, size)
                 SELECT ?1, path, hash, size FROM undo_files WHERE token = ?2",
                params![site_id, latest],
            )?;
            tx.execute(
                "INSERT INTO expiry_policies
                    (site_id, path, target_kind, mode, duration_seconds, deadline,
                     min_age_seconds, max_age_seconds, max_size_bytes, power,
                     refreshed, own_deadline, size_bytes)
                 SELECT ?1, path, target_kind, mode, duration_seconds, deadline,
                        min_age_seconds, max_age_seconds, max_size_bytes, power,
                        refreshed, own_deadline, size_bytes
                 FROM undo_expiry_policies WHERE token = ?2",
                params![site_id, latest],
            )?;
            regenerate_site(&tx, &self.inner.blob_files, site_id, snapshot.updated)?;
            tx.execute(
                "DELETE FROM management_tombstones
                 WHERE name IN (SELECT name FROM undo_names WHERE token = ?1)",
                [&latest],
            )?;
        }
        tx.execute(
            "UPDATE undo_operations SET consumed = 1 WHERE token = ?1",
            [&latest],
        )?;
        prune_undo_locked(&tx, now)?;
        let removed = gc_blobs(&tx, now)?;
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
        let tx = db.transaction()?;
        authorize_locked(&tx, name, authorization)?;
        let kind = expiry_target_kind_locked(&tx, name, &rel)?;
        let site_id: i64 = tx.query_row("SELECT id FROM sites WHERE name = ?1", [name], |row| {
            row.get(0)
        })?;
        let previous = tx
            .query_row(
                "SELECT 1 FROM expiry_policies WHERE site_id = ?1 AND path = ?2",
                params![site_id, rel],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let undo = if previous || policy.is_some() {
            Some(snapshot_site_with_description(
                &tx,
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
            let size = expiry_target_size_locked(&tx, site_id, &rel, kind)?;
            store_expiry_policy_locked(
                &tx,
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
            tx.execute(
                "DELETE FROM expiry_policies WHERE site_id = ?1 AND path = ?2",
                params![site_id, rel],
            )?;
        }
        regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&tx, now)?;
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

    pub fn expiry_report(&self, name: &str, rel: &str) -> Result<ExpiryReport, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let now = self.now_millis();
        let db = self.inner.readers.get();
        let kind = expiry_target_kind_locked(&db, name, &rel)?;
        let site_id: i64 = db.query_row("SELECT id FROM sites WHERE name = ?1", [name], |row| {
            row.get(0)
        })?;
        let size = expiry_target_size_locked(&db, site_id, &rel, kind)?;
        let own = load_expiry_policy_locked(&db, site_id, &rel)?;
        let mut inherited = Vec::new();
        for ancestor in expiry_ancestor_paths(&rel) {
            if let Some(stored) = load_expiry_policy_locked(&db, site_id, &ancestor)? {
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
        Ok(ExpiryReport {
            target: ExpiryTarget {
                site: name.to_string(),
                path: (!rel.is_empty()).then_some(rel),
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
        })
    }

    pub fn next_expiry_delay(&self) -> Result<std::time::Duration, StoreError> {
        let db = self.inner.readers.get();
        let next = db.query_row("SELECT MIN(own_deadline) FROM expiry_policies", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
        let millis = next.map_or(60_000, |deadline| {
            deadline.saturating_sub(self.now_millis()).clamp(0, 60_000)
        });
        Ok(std::time::Duration::from_millis(
            u64::try_from(millis).expect("delay is non-negative"),
        ))
    }

    pub fn sweep_expired(&self) -> Result<usize, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let due = {
            let mut stmt = tx.prepare(
                "SELECT sites.name, expiry_policies.path, expiry_policies.target_kind
                 FROM expiry_policies
                 JOIN sites ON sites.id = expiry_policies.site_id
                 WHERE expiry_policies.own_deadline <= ?1
                 ORDER BY sites.id, expiry_policies.target_kind, expiry_policies.path",
            )?;
            stmt.query_map([now], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut swept_sites = HashSet::new();
        let mut removed_targets = 0;
        for (name, path, raw_kind) in due {
            if !site_exists_locked(&tx, &name)? {
                continue;
            }
            let kind = ExpiryTargetKind::try_from(raw_kind)?;
            let site_id: i64 =
                tx.query_row("SELECT id FROM sites WHERE name = ?1", [&name], |row| {
                    row.get(0)
                })?;
            let still_due = tx
                .query_row(
                    "SELECT own_deadline <= ?3 FROM expiry_policies
                 WHERE site_id = ?1 AND path = ?2",
                    params![site_id, path, now],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false);
            if !still_due {
                continue;
            }
            if swept_sites.insert(name.clone()) {
                snapshot_site_with_description(
                    &tx,
                    &name,
                    UndoKind::ExpireSweep,
                    &format!("restore expired content in {name}"),
                    now,
                )?;
            }
            match kind {
                ExpiryTargetKind::Site => {
                    retain_management_tombstone(&tx, &name, now)?;
                    tx.execute("DELETE FROM sites WHERE id = ?1", [site_id])?;
                }
                ExpiryTargetKind::File => {
                    tx.execute(
                        "DELETE FROM files WHERE site_id = ?1 AND path = ?2",
                        params![site_id, path],
                    )?;
                    tx.execute(
                        "DELETE FROM expiry_policies WHERE site_id = ?1 AND path = ?2",
                        params![site_id, path],
                    )?;
                    finish_partial_expiry_locked(&tx, &self.inner.blob_files, site_id, &path, now)?;
                }
                ExpiryTargetKind::Folder => {
                    let (start, end) = descendant_bounds(&path);
                    tx.execute(
                        "DELETE FROM files WHERE site_id = ?1 AND path >= ?2 AND path < ?3",
                        params![site_id, start, end],
                    )?;
                    tx.execute(
                        "DELETE FROM expiry_policies
                         WHERE site_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
                        params![site_id, path, start, end],
                    )?;
                    finish_partial_expiry_locked(&tx, &self.inner.blob_files, site_id, &path, now)?;
                }
            }
            removed_targets += 1;
        }
        prune_undo_locked(&tx, now)?;
        let removed = gc_blobs(&tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(removed_targets)
    }

    fn backfill_manifests(&self) -> Result<(), StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let ids = {
            let mut stmt = tx.prepare("SELECT id FROM sites ORDER BY id")?;
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for site_id in ids {
            tx.execute(
                "UPDATE sites
                 SET public_url = ?1,
                     content_revision = CASE WHEN content_revision = 0 THEN 1 ELSE content_revision END
                 WHERE id = ?2",
                params![self.inner.public_url, site_id],
            )?;
            regenerate_site(&tx, &self.inner.blob_files, site_id, now)?;
        }
        tx.commit()?;
        drop(db);
        Ok(())
    }

    fn prune_undo_and_gc(&self) -> Result<(), StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        prune_undo_locked(&tx, now)?;
        prune_idempotency_locked(&tx, now)?;
        let removed = gc_blobs(&tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }

    fn now_millis(&self) -> i64 {
        self.inner.clock.now_millis()
    }

    fn tmp_dir(&self, name: &str) -> PathBuf {
        let t = self.now_millis();
        self.inner
            .root
            .join("tmp")
            .join(format!("{name}-{}-{t}", std::process::id()))
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
        }
    }

    const fn from_i64(value: i64) -> Self {
        match value {
            2 => Self::DeletePath,
            3 => Self::DeleteSite,
            4 => Self::Copy,
            5 => Self::Move,
            6 => Self::Expiry,
            7 => Self::ExpireSweep,
            _ => Self::Put,
        }
    }
}

#[derive(Clone, Copy)]
#[repr(i64)]
enum IdempotencyKind {
    UnnamedPut = 1,
    AutoCopy = 2,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PublishedMutation {
    name: String,
    mutation: MutationResult,
}

#[derive(Clone, Copy)]
struct MergeContext<'a> {
    name: &'a str,
    kind: UndoKind,
    expected_tree_hash: Option<&'a str>,
    now: i64,
    creation: CreationSecurity,
    authorization: Option<&'a ManagementToken>,
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

fn run_migrations(db: &mut Connection) -> Result<(), StoreError> {
    let mut version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        let tx = db.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
        version = 1;
    }
    if version == 1 {
        let collisions = {
            let mut stmt = db.prepare(
                "SELECT sites.name, files.path FROM files JOIN sites ON sites.id = files.site_id",
            )?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, path)| path != MANIFEST_PATH && is_reserved_path(path))
            .collect::<Vec<_>>()
        };
        if let Some((site, path)) = collisions.first() {
            return Err(StoreError::ReservedCollision(format!("{site}/{path}")));
        }
        migrate_lifecycle_schema(db)?;
        version = 3;
    }
    if version == 2 {
        migrate_expiry_schema(db)?;
        version = 3;
    }
    if version == 3 {
        migrate_management_schema(db)?;
        version = LATEST_SCHEMA_VERSION;
    }
    if version != LATEST_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported database schema version {version}"),
        )
        .into());
    }
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, integrity).into());
    }
    Ok(())
}

fn migrate_management_schema(db: &mut Connection) -> Result<(), StoreError> {
    let tx = db.transaction()?;
    tx.execute_batch(
        "ALTER TABLE sites ADD COLUMN creator_kind INTEGER;
         ALTER TABLE sites ADD COLUMN creator_hash BLOB;
         ALTER TABLE sites ADD COLUMN claim_hash BLOB;
         ALTER TABLE sites ADD COLUMN management_hash BLOB;
         ALTER TABLE sites ADD COLUMN management_status INTEGER NOT NULL DEFAULT 0;
         CREATE TABLE management_tombstones (
             name TEXT PRIMARY KEY,
             management_hash BLOB NOT NULL,
             created INTEGER NOT NULL
         );
         CREATE TABLE management_audit (
             id INTEGER PRIMARY KEY,
             site_name TEXT NOT NULL,
             action INTEGER NOT NULL,
             occurred INTEGER NOT NULL
         );
         CREATE TABLE management_idempotency (
             key_hash TEXT PRIMARY KEY,
             fingerprint TEXT NOT NULL,
             expires INTEGER NOT NULL
         );
         CREATE INDEX management_idempotency_expiry ON management_idempotency(expires);
         CREATE INDEX management_audit_site ON management_audit(site_name, occurred);",
    )?;
    tx.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn migrate_expiry_schema(db: &mut Connection) -> Result<(), StoreError> {
    let tx = db.transaction()?;
    tx.execute_batch(
        "ALTER TABLE expiry_policies ADD COLUMN size_bytes INTEGER NOT NULL DEFAULT 0;
         CREATE TABLE undo_expiry_policies (
             token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
             path TEXT NOT NULL,
             target_kind INTEGER NOT NULL,
             mode INTEGER NOT NULL,
             duration_seconds INTEGER,
             deadline INTEGER,
             min_age_seconds INTEGER,
             max_age_seconds INTEGER,
             max_size_bytes INTEGER,
             power REAL,
             refreshed INTEGER,
             own_deadline INTEGER NOT NULL,
             size_bytes INTEGER NOT NULL,
             PRIMARY KEY (token, path)
         );
         CREATE INDEX expiry_policies_site_kind
             ON expiry_policies(site_id, target_kind, path);",
    )?;
    tx.pragma_update(None, "user_version", 3)?;
    tx.commit()?;
    Ok(())
}

fn migrate_lifecycle_schema(db: &mut Connection) -> Result<(), StoreError> {
    let tx = db.transaction()?;
    tx.execute_batch(
        "ALTER TABLE sites ADD COLUMN public_url TEXT NOT NULL DEFAULT '';
             ALTER TABLE sites ADD COLUMN content_revision INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sites ADD COLUMN tree_hash TEXT NOT NULL DEFAULT '';
             CREATE TABLE undo_operations (
                 token TEXT PRIMARY KEY,
                 kind INTEGER NOT NULL,
                 description TEXT NOT NULL,
                 created INTEGER NOT NULL,
                 expires INTEGER NOT NULL,
                 consumed INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX undo_operations_retention
                 ON undo_operations(consumed, expires, created);
             CREATE TABLE undo_names (
                 token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 PRIMARY KEY (token, name)
             );
             CREATE INDEX undo_names_stack ON undo_names(name, token);
             CREATE TABLE undo_sites (
                 token TEXT PRIMARY KEY REFERENCES undo_operations(token) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 existed INTEGER NOT NULL,
                 public_url TEXT NOT NULL,
                 updated INTEGER NOT NULL,
                 content_revision INTEGER NOT NULL,
                 tree_hash TEXT NOT NULL
             );
             CREATE TABLE undo_files (
                 token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
                 path TEXT NOT NULL,
                 hash TEXT NOT NULL REFERENCES blobs(hash),
                 size INTEGER NOT NULL,
                 PRIMARY KEY (token, path)
             );
             CREATE INDEX undo_files_hash ON undo_files(hash);
             CREATE TABLE expiry_policies (
                 site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                 path TEXT NOT NULL,
                 target_kind INTEGER NOT NULL,
                 mode INTEGER NOT NULL,
                 duration_seconds INTEGER,
                 deadline INTEGER,
                 min_age_seconds INTEGER,
                 max_age_seconds INTEGER,
                 max_size_bytes INTEGER,
                 power REAL,
                 refreshed INTEGER,
                 own_deadline INTEGER,
                 size_bytes INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (site_id, path)
             );
             CREATE INDEX expiry_policies_deadline ON expiry_policies(own_deadline);
             CREATE INDEX expiry_policies_site_kind
                 ON expiry_policies(site_id, target_kind, path);
             CREATE TABLE undo_expiry_policies (
                 token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
                 path TEXT NOT NULL,
                 target_kind INTEGER NOT NULL,
                 mode INTEGER NOT NULL,
                 duration_seconds INTEGER,
                 deadline INTEGER,
                 min_age_seconds INTEGER,
                 max_age_seconds INTEGER,
                 max_size_bytes INTEGER,
                 power REAL,
                 refreshed INTEGER,
                 own_deadline INTEGER NOT NULL,
                 size_bytes INTEGER NOT NULL,
                 PRIMARY KEY (token, path)
             );
             CREATE TABLE idempotency_records (
                 key_hash TEXT PRIMARY KEY,
                 fingerprint TEXT NOT NULL,
                 operation_kind INTEGER NOT NULL,
                 result_metadata TEXT NOT NULL,
                 expires INTEGER NOT NULL
             );
             CREATE INDEX idempotency_records_expiry ON idempotency_records(expires);",
    )?;
    tx.pragma_update(None, "user_version", 3)?;
    tx.commit()?;
    Ok(())
}

fn reject_reserved_path(path: &str) -> Result<(), StoreError> {
    if is_reserved_path(path) {
        Err(StoreError::Upload(UploadError::ReservedPath))
    } else {
        Ok(())
    }
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

fn staged_fingerprint(files: &[&StagedFile]) -> String {
    let mut files = files.to_vec();
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-unnamed-put-v1\0");
    for file in files {
        hasher.update(&(file.path.len() as u64).to_le_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update(file.hash.as_bytes());
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
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
) -> Result<Option<PublishedMutation>, StoreError> {
    let key_hash = idempotency_key_hash(key);
    let record = tx
        .query_row(
            "SELECT fingerprint, operation_kind, result_metadata
             FROM idempotency_records WHERE key_hash = ?1",
            [key_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
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
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
    result: &PublishedMutation,
    now: i64,
) -> Result<(), StoreError> {
    let metadata = serde_json::to_string(result)
        .map_err(|err| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    tx.execute(
        "INSERT INTO idempotency_records
            (key_hash, fingerprint, operation_kind, result_metadata, expires)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            idempotency_key_hash(key),
            fingerprint,
            kind as i64,
            metadata,
            now + IDEMPOTENCY_RETENTION_MILLIS
        ],
    )?;
    Ok(())
}

fn prune_idempotency_locked(
    tx: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute("DELETE FROM idempotency_records WHERE expires <= ?1", [now])?;
    Ok(())
}

fn management_hash_from_blob(bytes: &[u8]) -> Result<ManagementTokenHash, StoreError> {
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid stored management hash")
    })?;
    Ok(ManagementTokenHash::from_bytes(bytes))
}

fn authorize_locked(
    db: &Connection,
    name: &str,
    token: Option<&ManagementToken>,
) -> Result<(), StoreError> {
    let live = db
        .query_row(
            "SELECT management_status, management_hash FROM sites WHERE name = ?1",
            [name],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .optional()?;
    let expected = match live {
        Some((false, _)) => return Ok(()),
        Some((true, hash)) => hash,
        None => db
            .query_row(
                "SELECT management_hash FROM management_tombstones WHERE name = ?1",
                [name],
                |row| row.get::<_, Vec<u8>>(0),
            )
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
    tx: &rusqlite::Transaction<'_>,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<bool, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(false);
    };
    validate_idempotency_key(&idempotency.key)?;
    let stored = tx
        .query_row(
            "SELECT fingerprint FROM management_idempotency WHERE key_hash = ?1",
            [idempotency_key_hash(&idempotency.key)],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match stored {
        Some(stored) if stored == fingerprint => Ok(true),
        Some(_) => Err(StoreError::IdempotencyConflict),
        None => Ok(false),
    }
}

fn store_management_idempotency(
    tx: &rusqlite::Transaction<'_>,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO management_idempotency(key_hash, fingerprint, expires)
         VALUES (?1, ?2, ?3)",
        params![
            idempotency_key_hash(&idempotency.key),
            fingerprint,
            now + IDEMPOTENCY_RETENTION_MILLIS
        ],
    )?;
    Ok(())
}

fn prune_management_idempotency(
    tx: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "DELETE FROM management_idempotency WHERE expires <= ?1",
        [now],
    )?;
    Ok(())
}

fn record_management(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    action: i64,
    now: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO management_audit(site_name, action, occurred) VALUES (?1, ?2, ?3)",
        params![name, action, now],
    )?;
    Ok(())
}

fn snapshot_site(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    kind: UndoKind,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let description = match kind {
        UndoKind::Put => format!("restore previous state of {name}"),
        UndoKind::DeletePath | UndoKind::DeleteSite => format!("restore deleted site {name}"),
        UndoKind::Copy => format!("remove copied site {name}"),
        UndoKind::Move => format!("restore previous name {name}"),
        UndoKind::Expiry => format!("restore previous expiry policy for {name}"),
        UndoKind::ExpireSweep => format!("restore expired content in {name}"),
    };
    snapshot_site_with_description(tx, name, kind, &description, now)
}

fn snapshot_site_with_description(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    kind: UndoKind,
    description: &str,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let token = undo_token()?;
    let expires = now + UNDO_RETENTION_MILLIS;
    tx.execute(
        "INSERT INTO undo_operations(token, kind, description, created, expires)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token, kind as i64, description, now, expires],
    )?;
    tx.execute(
        "INSERT INTO undo_names(token, name) VALUES (?1, ?2)",
        params![token, name],
    )?;
    let site = tx
        .query_row(
            "SELECT name, public_url, updated, content_revision, tree_hash
             FROM sites WHERE name = ?1",
            [name],
            |row| {
                Ok(SiteSnapshot {
                    name: row.get(0)?,
                    existed: true,
                    public_url: row.get(1)?,
                    updated: row.get(2)?,
                    content_revision: row.get(3)?,
                    tree_hash: row.get(4)?,
                })
            },
        )
        .optional()?
        .unwrap_or_else(|| SiteSnapshot {
            name: name.to_string(),
            existed: false,
            public_url: String::new(),
            updated: now,
            content_revision: 0,
            tree_hash: String::new(),
        });
    tx.execute(
        "INSERT INTO undo_sites
            (token, name, existed, public_url, updated, content_revision, tree_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            token,
            site.name,
            site.existed,
            site.public_url,
            site.updated,
            site.content_revision,
            site.tree_hash
        ],
    )?;
    if site.existed {
        tx.execute(
            "INSERT INTO undo_files(token, path, hash, size)
             SELECT ?1, path, hash, size FROM files
             WHERE site_id = (SELECT id FROM sites WHERE name = ?2) AND path <> ?3",
            params![token, name, MANIFEST_PATH],
        )?;
        tx.execute(
            "INSERT INTO undo_expiry_policies
                (token, path, target_kind, mode, duration_seconds, deadline,
                 min_age_seconds, max_age_seconds, max_size_bytes, power,
                 refreshed, own_deadline, size_bytes)
             SELECT ?1, path, target_kind, mode, duration_seconds, deadline,
                    min_age_seconds, max_age_seconds, max_size_bytes, power,
                    refreshed, own_deadline, size_bytes
             FROM expiry_policies
             WHERE site_id = (SELECT id FROM sites WHERE name = ?2)",
            params![token, name],
        )?;
    }
    Ok(UndoInfo {
        token,
        expires_at: format_timestamp(expires),
    })
}

fn expiry_display_path(name: &str, path: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{name}/{path}")
    }
}

fn expiry_target_kind_locked(
    db: &Connection,
    name: &str,
    rel: &str,
) -> Result<ExpiryTargetKind, StoreError> {
    match node_locked(db, name, rel)? {
        NodeKind::Missing => Err(StoreError::NotFound),
        NodeKind::Dir if rel.is_empty() => Ok(ExpiryTargetKind::Site),
        NodeKind::Dir => Ok(ExpiryTargetKind::Folder),
        NodeKind::File { .. } => Ok(ExpiryTargetKind::File),
    }
}

fn expiry_target_size_locked(
    db: &Connection,
    site_id: i64,
    rel: &str,
    kind: ExpiryTargetKind,
) -> Result<u64, StoreError> {
    let size: i64 = match kind {
        ExpiryTargetKind::Site => db.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM files
             WHERE site_id = ?1 AND path <> ?2",
            params![site_id, MANIFEST_PATH],
            |row| row.get(0),
        )?,
        ExpiryTargetKind::File => db.query_row(
            "SELECT size FROM files WHERE site_id = ?1 AND path = ?2",
            params![site_id, rel],
            |row| row.get(0),
        )?,
        ExpiryTargetKind::Folder => {
            let (start, end) = descendant_bounds(rel);
            db.query_row(
                "SELECT COALESCE(SUM(size), 0) FROM files
                 WHERE site_id = ?1 AND path >= ?2 AND path < ?3 AND path <> ?4",
                params![site_id, start, end, MANIFEST_PATH],
                |row| row.get(0),
            )?
        }
    };
    Ok(size.cast_unsigned())
}

#[allow(clippy::too_many_lines)]
fn store_expiry_policy_locked(
    tx: &rusqlite::Transaction<'_>,
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
    tx.execute(
        "INSERT INTO expiry_policies
            (site_id, path, target_kind, mode, duration_seconds, deadline,
             min_age_seconds, max_age_seconds, max_size_bytes, power,
             refreshed, own_deadline, size_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(site_id, path) DO UPDATE SET
             target_kind = excluded.target_kind,
             mode = excluded.mode,
             duration_seconds = excluded.duration_seconds,
             deadline = excluded.deadline,
             min_age_seconds = excluded.min_age_seconds,
             max_age_seconds = excluded.max_age_seconds,
             max_size_bytes = excluded.max_size_bytes,
             power = excluded.power,
             refreshed = excluded.refreshed,
             own_deadline = excluded.own_deadline,
             size_bytes = excluded.size_bytes",
        params![
            site_id,
            path,
            i64::from(kind),
            i64::from(policy.mode()),
            duration,
            deadline,
            min_age,
            max_age,
            max_size,
            power,
            refreshed,
            own_deadline,
            i64::try_from(size).map_err(|_| ExpiryError::DeadlineOverflow)?,
        ],
    )?;
    Ok(())
}

fn load_expiry_policy_locked(
    db: &Connection,
    site_id: i64,
    path: &str,
) -> Result<Option<StoredExpiryPolicy>, StoreError> {
    let row = db
        .query_row(
            "SELECT target_kind, mode, duration_seconds, deadline,
                    min_age_seconds, max_age_seconds, max_size_bytes, power,
                    refreshed, own_deadline, size_bytes
             FROM expiry_policies WHERE site_id = ?1 AND path = ?2",
            params![site_id, path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        raw_kind,
        raw_mode,
        duration,
        deadline,
        min_age,
        max_age,
        max_size,
        power,
        refreshed,
        own_deadline,
        size,
    )) = row
    else {
        return Ok(None);
    };
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
    tx: &rusqlite::Transaction<'_>,
    source_id: i64,
    destination_id: i64,
    now: i64,
) -> Result<(), StoreError> {
    let paths = {
        let mut stmt =
            tx.prepare("SELECT path FROM expiry_policies WHERE site_id = ?1 ORDER BY path")?;
        stmt.query_map([source_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
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
    tx: &rusqlite::Transaction<'_>,
    site_id: i64,
    changed_paths: &[&str],
    now: i64,
) -> Result<(), StoreError> {
    let policies = {
        let mut stmt = tx.prepare(
            "SELECT path, target_kind FROM expiry_policies
             WHERE site_id = ?1 AND mode IN (?2, ?3)",
        )?;
        stmt.query_map(
            params![
                site_id,
                i64::from(ExpiryMode::Relative),
                i64::from(ExpiryMode::Decay)
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (path, raw_kind) in policies {
        let kind = ExpiryTargetKind::try_from(raw_kind)?;
        if !changed_paths
            .iter()
            .any(|changed| policy_is_affected(&path, kind, changed))
        {
            continue;
        }
        let Some(stored) = load_expiry_policy_locked(tx, site_id, &path)? else {
            continue;
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
    tx: &rusqlite::Transaction<'_>,
    blobs: &BlobFiles,
    site_id: i64,
    changed_path: &str,
    now: i64,
) -> Result<(), StoreError> {
    let remaining: i64 = tx.query_row(
        "SELECT COUNT(*) FROM files WHERE site_id = ?1 AND path <> ?2",
        params![site_id, MANIFEST_PATH],
        |row| row.get(0),
    )?;
    if remaining == 0 {
        let name: String =
            tx.query_row("SELECT name FROM sites WHERE id = ?1", [site_id], |row| {
                row.get(0)
            })?;
        retain_management_tombstone(tx, &name, now)?;
        tx.execute("DELETE FROM sites WHERE id = ?1", [site_id])?;
        return Ok(());
    }
    tx.execute(
        "UPDATE sites SET content_revision = content_revision + 1 WHERE id = ?1",
        [site_id],
    )?;
    refresh_expiry_for_changes_locked(tx, site_id, &[changed_path], now)?;
    regenerate_site(tx, blobs, site_id, now)?;
    Ok(())
}

fn regenerate_site(
    tx: &rusqlite::Transaction<'_>,
    blobs: &BlobFiles,
    site_id: i64,
    updated: i64,
) -> Result<String, StoreError> {
    let (name, public_url, revision, managed): (String, String, i64, bool) = tx.query_row(
        "SELECT name, public_url, content_revision, management_status FROM sites WHERE id = ?1",
        [site_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let entries = {
        let mut stmt = tx.prepare(
            "SELECT path, hash FROM files
             WHERE site_id = ?1 AND path <> ?2 ORDER BY path",
        )?;
        stmt.query_map(params![site_id, MANIFEST_PATH], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in &entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(hash.as_bytes());
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
    let expiry_paths = {
        let mut stmt = tx.prepare(
            "SELECT path FROM expiry_policies
             WHERE site_id = ?1
             ORDER BY target_kind, path",
        )?;
        stmt.query_map([site_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
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
    tx.execute(
        "INSERT OR IGNORE INTO blobs(hash, bytes, size) VALUES (?1, X'', ?2)",
        params![staged.hash, staged.size],
    )?;
    tx.execute(
        "INSERT INTO files(site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(site_id, path) DO UPDATE SET hash = excluded.hash, size = excluded.size",
        params![site_id, MANIFEST_PATH, staged.hash, staged.size],
    )?;
    tx.execute(
        "UPDATE sites SET tree_hash = ?1, updated = ?2 WHERE id = ?3",
        params![tree_hash, updated, site_id],
    )?;
    Ok(tree_hash)
}

const fn expiry_mode_name(mode: ExpiryMode) -> &'static str {
    match mode {
        ExpiryMode::Relative => "relative",
        ExpiryMode::Absolute => "absolute",
        ExpiryMode::Decay => "decay",
    }
}

fn site_revision_locked(db: &Connection, name: &str) -> Result<(u64, String), StoreError> {
    db.query_row(
        "SELECT content_revision, tree_hash FROM sites WHERE name = ?1",
        [name],
        |row| Ok((row.get::<_, i64>(0)?.cast_unsigned(), row.get(1)?)),
    )
    .map_err(map_sql)
}

fn retain_management_tombstone(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    now: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO management_tombstones(name, management_hash, created)
         SELECT name, management_hash, ?2 FROM sites
         WHERE name = ?1 AND management_status = 1 AND management_hash IS NOT NULL
         ON CONFLICT(name) DO UPDATE SET
             management_hash = excluded.management_hash,
             created = excluded.created",
        params![name, now],
    )?;
    Ok(())
}

fn prune_undo_locked(tx: &rusqlite::Transaction<'_>, now: i64) -> Result<(), rusqlite::Error> {
    tx.execute(
        "DELETE FROM undo_operations WHERE consumed = 1 OR expires <= ?1",
        [now],
    )?;
    tx.execute(
        "DELETE FROM undo_operations
         WHERE token IN (
             SELECT token FROM (
                 SELECT names.token,
                        ROW_NUMBER() OVER (
                            PARTITION BY names.name
                            ORDER BY operation.created DESC, operation.rowid DESC
                        ) AS position
                 FROM undo_names AS names
                 JOIN undo_operations AS operation ON operation.token = names.token
             ) WHERE position > ?1
         )",
        [UNDO_LIMIT_PER_SITE],
    )?;
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

fn site_exists_locked(db: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
    db.query_row("SELECT 1 FROM sites WHERE name = ?1", params![name], |_| {
        Ok(())
    })
    .optional()
    .map(|row| row.is_some())
}

fn node_locked(db: &Connection, name: &str, rel: &str) -> Result<NodeKind, StoreError> {
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
    let (prefix_start, prefix_end) = descendant_bounds(rel);
    let (site_exists, hash, dir_exists): (bool, Option<String>, bool) = db.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM sites WHERE name = ?1),
            (
                SELECT hash FROM files
                WHERE site_id = (SELECT id FROM sites WHERE name = ?1)
                  AND path = ?2
            ),
            EXISTS(
                SELECT 1 FROM files
                WHERE site_id = (SELECT id FROM sites WHERE name = ?1)
                  AND path >= ?3
                  AND path < ?4
            )",
        params![name, rel, prefix_start, prefix_end],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(if !site_exists {
        NodeKind::Missing
    } else if let Some(hash) = hash {
        NodeKind::File { hash }
    } else if dir_exists {
        NodeKind::Dir
    } else {
        NodeKind::Missing
    })
}

fn load_root_files(db: &Connection, name: &str) -> Result<Vec<(String, u64)>, rusqlite::Error> {
    let mut stmt = db.prepare(
        "SELECT path, size
         FROM files
         WHERE site_id = (SELECT id FROM sites WHERE name = ?1)
         ORDER BY path",
    )?;
    let rows = stmt.query_map(params![name], file_path_and_size)?;
    rows.collect()
}

fn load_descendant_files(
    db: &Connection,
    name: &str,
    rel: &str,
) -> Result<Vec<(String, u64)>, rusqlite::Error> {
    let (prefix_start, prefix_end) = descendant_bounds(rel);
    let mut stmt = db.prepare(
        "SELECT path, size
         FROM files
         WHERE site_id = (SELECT id FROM sites WHERE name = ?1)
           AND path >= ?2
           AND path < ?3
         ORDER BY path",
    )?;
    let rows = stmt.query_map(params![name, prefix_start, prefix_end], file_path_and_size)?;
    rows.collect()
}

fn file_path_and_size(row: &rusqlite::Row<'_>) -> Result<(String, u64), rusqlite::Error> {
    Ok((row.get(0)?, row.get::<_, i64>(1)?.cast_unsigned()))
}

fn descendant_bounds(rel: &str) -> (String, String) {
    (format!("{rel}/"), format!("{rel}0"))
}

fn load_sizes(db: &Connection, sql: &str) -> Result<Vec<u64>, rusqlite::Error> {
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0).map(i64::cast_unsigned))?;
    rows.collect()
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
fn site_files(db: &Connection, blobs: &BlobFiles, name: &str) -> Result<SiteArchive, StoreError> {
    let entries = site_manifest(db, name)?;
    let mut files = Vec::new();
    for entry in entries {
        let bytes = blobs.read(&entry.hash)?;
        if !is_junk(Path::new(&entry.path), Some(&bytes)) {
            files.push(ArchiveFile {
                path: entry.path,
                bytes,
            });
        }
    }
    Ok(SiteArchive { files })
}

fn site_manifest(db: &Connection, name: &str) -> Result<Vec<ArchiveEntry>, StoreError> {
    let site_id: i64 = db
        .query_row(
            "SELECT id FROM sites WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    let mut stmt = db.prepare(
        "SELECT path, hash, size
         FROM files
         WHERE site_id = ?1
         ORDER BY path",
    )?;
    let rows = stmt.query_map(params![site_id], |row| {
        Ok(ArchiveEntry {
            path: row.get(0)?,
            hash: row.get(1)?,
            size: row.get::<_, i64>(2)?.cast_unsigned(),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.size);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(
            &mut header,
            &file.path,
            fs::File::open(blobs.path(&file.hash))?,
        )?;
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
    for file in files {
        archive
            .start_file(&file.path, options)
            .map_err(io::Error::other)?;
        io::copy(&mut fs::File::open(blobs.path(&file.hash))?, &mut archive)?;
    }
    archive.finish().map(|_| ()).map_err(io::Error::other)
}

#[cfg(test)]
fn append_tar<W: Write>(writer: W, files: &[ArchiveFile]) -> io::Result<W> {
    let mut archive = tar::Builder::new(writer);
    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, &file.path, file.bytes.as_slice())?;
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
        archive
            .start_file(&file.path, options)
            .map_err(io::Error::other)?;
        archive.write_all(&file.bytes)?;
    }
    archive
        .finish()
        .map(Cursor::into_inner)
        .map_err(io::Error::other)
}

fn gc_blobs(tx: &rusqlite::Transaction<'_>, now: i64) -> Result<Vec<String>, rusqlite::Error> {
    let hashes = {
        let mut stmt = tx.prepare(
            "SELECT hash FROM blobs
             WHERE hash NOT IN (SELECT DISTINCT hash FROM files)
               AND hash NOT IN (
                   SELECT DISTINCT files.hash
                   FROM undo_files AS files
                   JOIN undo_operations AS operation ON operation.token = files.token
                   WHERE operation.consumed = 0 AND operation.expires > ?1
               )",
        )?;
        stmt.query_map([now], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    tx.execute(
        "DELETE FROM blobs
         WHERE hash NOT IN (SELECT DISTINCT hash FROM files)
           AND hash NOT IN (
               SELECT DISTINCT files.hash
               FROM undo_files AS files
               JOIN undo_operations AS operation ON operation.token = files.token
               WHERE operation.consumed = 0 AND operation.expires > ?1
           )",
        [now],
    )?;
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

fn map_sql(err: rusqlite::Error) -> StoreError {
    match err {
        rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound,
        other => StoreError::Sqlite(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestClock {
        millis: AtomicU64,
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
        let db = Connection::open(dir.path().join("symbol.db")).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            LATEST_SCHEMA_VERSION
        );
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
        let db = Connection::open(dir.path().join("symbol.db")).unwrap();
        let stored_bytes: i64 = db
            .query_row(
                "SELECT length(bytes) FROM blobs WHERE hash = ?1",
                [&hash],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_bytes, 0);
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
            let db = Connection::open(dir.path().join("symbol.db")).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute(
                "INSERT INTO blobs (hash, bytes, size) VALUES (?1, ?2, 6)",
                params![hash, b"legacy".as_slice()],
            )
            .unwrap();
            db.execute("INSERT INTO sites (name, updated) VALUES ('hello', 0)", [])
                .unwrap();
            db.execute(
                "INSERT INTO files (site_id, path, hash, size)
                 VALUES ((SELECT id FROM sites WHERE name = 'hello'), 'legacy.bin', ?1, 6)",
                [&hash],
            )
            .unwrap();
        }
        let target = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"broken").unwrap();

        let store = Store::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(fs::read(store.blob_path(&hash)).unwrap(), b"legacy");
        assert_eq!(store.read_blob(&hash).unwrap(), "legacy");
        let db = Connection::open(dir.path().join("symbol.db")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT length(bytes) FROM blobs WHERE hash = ?1",
                [&hash],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT value FROM metadata WHERE key = 'external_blobs_v1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "1"
        );
    }

    #[test]
    fn startup_removes_orphaned_blob_files() {
        let dir = tempfile::tempdir().unwrap();
        let live_hash;
        {
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store.put_file("hello", "live.bin", b"live").unwrap();
            let Node::File { hash, .. } = store.lookup("hello", "live.bin").unwrap() else {
                panic!("expected file");
            };
            live_hash = hash;
        }
        let orphan_hash = "aa00000000000000000000000000000000000000000000000000000000000000";
        let orphan = dir
            .path()
            .join("blobs")
            .join(&orphan_hash[..2])
            .join(&orphan_hash[2..]);
        fs::create_dir_all(orphan.parent().unwrap()).unwrap();
        fs::write(&orphan, b"orphan").unwrap();

        let store = Store::new(dir.path().to_path_buf()).unwrap();
        assert!(!orphan.exists());
        assert_eq!(fs::read(store.blob_path(&live_hash)).unwrap(), b"live");
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
    fn startup_sweeps_junk() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::new(dir.path().to_path_buf()).unwrap();
            store
                .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
                .unwrap();
        }
        {
            let db = Connection::open(dir.path().join("symbol.db")).unwrap();
            let apple = [0x00u8, 0x05, 0x16, 0x07, 0, 2, 0, 0];
            let apple_len = i64::try_from(apple.len()).unwrap();
            let hash = blake3::hash(&apple).to_hex().to_string();
            let blob = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
            fs::create_dir_all(blob.parent().unwrap()).unwrap();
            fs::write(blob, apple).unwrap();
            db.execute(
                "INSERT INTO blobs (hash, bytes, size) VALUES (?1, X'', ?2)",
                params![hash, apple_len],
            )
            .unwrap();
            let site_id: i64 = db
                .query_row("SELECT id FROM sites WHERE name = 'hello'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            db.execute(
                "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)",
                params![site_id, "._index.html", hash, apple_len],
            )
            .unwrap();
            db.execute(
                "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)",
                params![site_id, "keep.bin", hash, apple_len],
            )
            .unwrap();
        }
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            store.list_files("hello").unwrap(),
            vec!["index.html".to_string(), "symbol.toml".to_string()]
        );
        assert!(matches!(
            store.lookup("hello", "._index.html").unwrap_err(),
            StoreError::NotFound
        ));
        let names: Vec<_> = store
            .list_dir("hello", "")
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec!["index.html".to_string(), "symbol.toml".to_string()]
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
        assert_eq!(sites.files, 5);
        assert!(sites.bytes > 42);
        assert_eq!(sites.entries[0].files, 5);
        assert!(sites.entries[0].bytes > 42);
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
}
