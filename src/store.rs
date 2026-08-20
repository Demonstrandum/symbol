use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::name::{NameError, generate_id, parse_site_name};
use crate::pathutil::{PathError, is_junk, is_noise_path, looks_like_apple_fork, safe_rel_path};
use crate::upload::{Kind, UploadError, write_payload};

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
    #[error("error: sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
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
    pub fn new(root: PathBuf) -> Result<Self, StoreError> {
        fs::create_dir_all(&root)?;
        let tmp = root.join("tmp");
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        let path = root.join("symbol.db");
        let db = Connection::open(&path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.execute_batch(SCHEMA)?;
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
            }),
        };
        store.migrate_sqlite_blobs()?;
        store.migrate_legacy()?;
        store.gc_junk()?;
        store.gc_blob_files()?;
        Ok(store)
    }

    pub fn blocking_capacity(&self) -> usize {
        self.inner.readers.size() + 1
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
        let file_values = load_sizes(&db, "SELECT size FROM files ORDER BY size")?;
        let blob_values = load_sizes(&db, "SELECT size FROM blobs ORDER BY size")?;
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

    pub fn site_exists(&self, name: &str) -> bool {
        let Ok(name) = parse_site_name(name) else {
            return false;
        };
        let db = self.inner.readers.get();
        site_exists_locked(&db, name).unwrap_or(false)
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

    pub fn publish(
        &self,
        wanted: Option<&str>,
        unpack: bool,
        bytes: &[u8],
        kind: Kind,
        filename: Option<&str>,
    ) -> Result<(String, usize), StoreError> {
        let name = match wanted {
            None | Some("") => generate_id(|candidate| self.site_exists(candidate)),
            Some(name) => parse_site_name(name)?.to_string(),
        };
        let n = self.replace_site(&name, bytes, kind, filename, unpack)?;
        Ok((name, n))
    }

    pub fn publish_uploaded_file(
        &self,
        wanted: Option<&str>,
        filename: &str,
        source: PathBuf,
    ) -> Result<(String, usize), StoreError> {
        let name = match wanted {
            None | Some("") => generate_id(|candidate| self.site_exists(candidate)),
            Some(name) => parse_site_name(name)?.to_string(),
        };
        let rel = safe_rel_path(filename)?
            .to_string_lossy()
            .replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        self.commit_site(&name, &[staged])?;
        Ok((name, 1))
    }

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
        let result = self.commit_site(&name, &staged);
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
        self.upsert_file(&name, &staged)
    }

    pub fn put_uploaded_file(
        &self,
        name: &str,
        rel: &str,
        source: PathBuf,
    ) -> Result<(), StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        self.upsert_file(&name, &staged)
    }

    #[cfg(test)]
    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let archive = site_files(&tx, &self.inner.blob_files, name)?;
        let packed = pack_tar_gz(&archive.files)?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        let removed = gc_blobs(&tx)?;
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

    pub fn pop_site_to_path(&self, name: &str, output: &Path) -> Result<u64, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let entries = site_manifest(&db, name)?;
        write_site_archive(
            &self.inner.blob_files,
            &entries,
            ArchiveFormat::TarGz,
            output,
        )?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        let removed = gc_blobs(&tx)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(fs::metadata(output)?.len())
    }

    pub fn delete_file(&self, name: &str, rel: &str) -> Result<(), StoreError> {
        let name = parse_site_name(name)?;
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let (prefix_start, prefix_end) = descendant_bounds(&rel);
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        let site_id: i64 = tx
            .query_row(
                "SELECT id FROM sites WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        let deleted = tx.execute(
            "DELETE FROM files
             WHERE site_id = ?1
               AND (path = ?2 OR (path >= ?3 AND path < ?4))",
            params![site_id, rel, prefix_start, prefix_end],
        )?;
        if deleted == 0 {
            return Err(StoreError::NotFound);
        }
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM files WHERE site_id = ?1",
            params![site_id],
            |row| row.get(0),
        )?;
        if remaining == 0 {
            tx.execute("DELETE FROM sites WHERE id = ?1", params![site_id])?;
        } else {
            tx.execute(
                "UPDATE sites SET updated = ?1 WHERE id = ?2",
                params![now_millis(), site_id],
            )?;
        }
        let removed = gc_blobs(&tx)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }

    fn commit_site(&self, name: &str, files: &[StagedFile]) -> Result<(), StoreError> {
        for file in files {
            self.materialize(file)?;
        }
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        for file in files {
            tx.execute(
                "INSERT OR IGNORE INTO blobs (hash, bytes, size) VALUES (?1, X'', ?2)",
                params![file.hash, file.size],
            )?;
        }
        tx.execute(
            "INSERT INTO sites (name, updated) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET updated = excluded.updated",
            params![name, now_millis()],
        )?;
        let site_id: i64 = tx.query_row(
            "SELECT id FROM sites WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        tx.execute("DELETE FROM files WHERE site_id = ?1", params![site_id])?;
        {
            let mut ins = tx
                .prepare("INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)")?;
            for file in files {
                ins.execute(params![site_id, file.path, file.hash, file.size])?;
            }
        }
        let removed = gc_blobs(&tx)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }

    fn upsert_file(&self, name: &str, file: &StagedFile) -> Result<(), StoreError> {
        self.materialize(file)?;
        let mut db = self.inner.writer.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, bytes, size) VALUES (?1, X'', ?2)",
            params![file.hash, file.size],
        )?;
        tx.execute(
            "INSERT INTO sites (name, updated) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET updated = excluded.updated",
            params![name, now_millis()],
        )?;
        let site_id: i64 = tx.query_row(
            "SELECT id FROM sites WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(site_id, path) DO UPDATE SET hash = excluded.hash, size = excluded.size",
            params![site_id, file.path, file.hash, file.size],
        )?;
        let removed = gc_blobs(&tx)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
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
        let removed = gc_blobs(&tx)?;
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

    fn tmp_dir(&self, name: &str) -> PathBuf {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        self.inner
            .root
            .join("tmp")
            .join(format!("{name}-{}-{t}", std::process::id()))
    }
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
    let hash = blake3::hash(bytes);
    StagedFile {
        path: path.to_string(),
        size: i64::try_from(bytes.len()).expect("file size fits in SQLite INTEGER"),
        hash: hash.to_hex().to_string(),
        source: StagedSource::Bytes(bytes.to_vec()),
    }
}

fn stage_file(path: &str, source: PathBuf) -> io::Result<Option<StagedFile>> {
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

fn gc_blobs(tx: &rusqlite::Transaction<'_>) -> Result<Vec<String>, rusqlite::Error> {
    let hashes = {
        let mut stmt = tx.prepare(
            "SELECT hash FROM blobs WHERE hash NOT IN (SELECT DISTINCT hash FROM files)",
        )?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    tx.execute(
        "DELETE FROM blobs WHERE hash NOT IN (SELECT DISTINCT hash FROM files)",
        [],
    )?;
    Ok(hashes)
}

fn normalize_rel(rel: &str) -> Result<String, StoreError> {
    if rel.is_empty() {
        return Ok(String::new());
    }
    Ok(safe_rel_path(rel)?.to_string_lossy().replace('\\', "/"))
}

fn now_millis() -> i64 {
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
            vec!["index.html".to_string()]
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
        assert!(matches!(
            store.read_blob(&hash).unwrap_err(),
            StoreError::NotFound
        ));
        assert!(!blob_path.exists());
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
            vec!["index.html".to_string()]
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
            vec!["index.html".to_string()]
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
        assert_eq!(names, vec!["index.html".to_string()]);
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
        assert_eq!(root.files, 4);
        assert_eq!(root.bytes, 42);
        assert_eq!(root.entries.len(), 3);
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
        assert!(!store.inner.blobs.contains(&hash));
        assert!(matches!(
            store.read_blob(&hash).unwrap_err(),
            StoreError::NotFound
        ));
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

        assert_eq!(store.list_files("hello").unwrap().len(), 41);
    }
}
