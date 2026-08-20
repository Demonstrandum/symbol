use std::collections::HashSet;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::{Connection, OptionalExtension, params};

use crate::name::{NameError, generate_id, parse_site_name};
use crate::pathutil::{PathError, is_junk, is_noise_path, looks_like_apple_fork, safe_rel_path};
use crate::upload::{Kind, UploadError, write_payload};

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS sites (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
    hash TEXT PRIMARY KEY,
    bytes BLOB NOT NULL,
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
";

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    db: Mutex<Connection>,
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
    bytes: Vec<u8>,
}

struct ArchiveFile {
    path: String,
    bytes: Vec<u8>,
}

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

impl Store {
    pub fn new(root: PathBuf) -> Result<Self, StoreError> {
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("tmp"))?;
        let db = Connection::open(root.join("symbol.db"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.execute_batch(SCHEMA)?;
        let store = Self {
            inner: Arc::new(Inner {
                root,
                db: Mutex::new(db),
            }),
        };
        store.migrate_legacy()?;
        store.gc_junk()?;
        Ok(store)
    }

    pub fn stats(&self) -> Result<Stats, StoreError> {
        let db = self.inner.db.lock().unwrap();
        let sites = db.query_row("SELECT COUNT(*) FROM sites", [], |row| {
            row.get::<_, i64>(0).map(|n| n as u64)
        })?;
        let file_values = load_sizes(&db, "SELECT size FROM files ORDER BY size")?;
        let blob_values = load_sizes(&db, "SELECT size FROM blobs ORDER BY size")?;
        let files = file_values.len() as u64;
        let blobs = blob_values.len() as u64;
        let logical_bytes = file_values.iter().sum();
        let bytes = blob_values.iter().sum();
        let saved_bytes = logical_bytes - bytes;
        let saved_fraction = if logical_bytes == 0 {
            0.0
        } else {
            saved_bytes as f64 / logical_bytes as f64
        };
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
        })
    }

    pub fn list_sites(&self) -> Result<SiteList, StoreError> {
        let db = self.inner.db.lock().unwrap();
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
                    files: row.get::<_, i64>(1)? as u64,
                    bytes: row.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
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
        let db = self.inner.db.lock().unwrap();
        if !site_exists_locked(&db, name)? {
            return Err(StoreError::NotFound);
        }
        let mut stmt = db.prepare(
            "SELECT path FROM files WHERE site_id = (SELECT id FROM sites WHERE name = ?1) ORDER BY path",
        )?;
        let paths = stmt
            .query_map(params![name], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(paths)
    }

    pub fn list_dir(&self, name: &str, rel: &str) -> Result<DirList, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        if !rel.is_empty() && is_noise_path(Path::new(&rel)) {
            return Err(StoreError::NotFound);
        }
        let db = self.inner.db.lock().unwrap();
        match node_locked(&db, name, &rel)? {
            NodeKind::Dir => {}
            NodeKind::File | NodeKind::Missing => return Err(StoreError::NotFound),
        }
        let mut stmt = db.prepare(
            "SELECT path, size FROM files WHERE site_id = (SELECT id FROM sites WHERE name = ?1) ORDER BY path",
        )?;
        let rows = stmt.query_map(params![name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        let mut files = Vec::new();
        for row in rows {
            files.push(row?);
        }
        Ok(dirents(&files, &rel))
    }

    pub fn site_exists(&self, name: &str) -> bool {
        let Ok(name) = parse_site_name(name) else {
            return false;
        };
        let db = self.inner.db.lock().unwrap();
        site_exists_locked(&db, name).unwrap_or(false)
    }

    pub fn lookup(&self, name: &str, rel: &str) -> Result<Node, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        if !rel.is_empty() && is_noise_path(Path::new(&rel)) {
            return Err(StoreError::NotFound);
        }
        let db = self.inner.db.lock().unwrap();
        match node_locked(&db, name, &rel)? {
            NodeKind::Missing => Err(StoreError::NotFound),
            NodeKind::Dir => Ok(Node::Dir),
            NodeKind::File => {
                let hash: String = db.query_row(
                    "SELECT hash FROM files WHERE site_id = (SELECT id FROM sites WHERE name = ?1) AND path = ?2",
                    params![name, rel],
                    |row| row.get(0),
                )?;
                Ok(Node::File { logical: rel, hash })
            }
        }
    }

    pub fn child_blob(&self, name: &str, rel: &str, child: &str) -> Result<Node, StoreError> {
        let path = if rel.is_empty() {
            child.to_string()
        } else {
            format!("{rel}/{child}")
        };
        self.lookup(name, &path)
    }

    pub fn read_blob(&self, hash: &str) -> Result<Vec<u8>, StoreError> {
        let db = self.inner.db.lock().unwrap();
        db.query_row("SELECT bytes FROM blobs WHERE hash = ?1", [hash], |row| {
            row.get(0)
        })
        .map_err(map_sql)
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
        let _ = fs::remove_dir_all(&tmp);
        self.commit_site(&name, &staged)?;
        Ok(n)
    }

    pub fn put_file(&self, name: &str, rel: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        if is_junk(Path::new(&rel), Some(bytes)) {
            return Err(UploadError::Junk.into());
        }
        let staged = stage_bytes(&rel, bytes);
        self.upsert_file(&name, &staged)
    }

    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        let archive = site_files(&tx, name)?;
        let packed = pack_tar_gz(&archive.files)?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        gc_blobs(&tx)?;
        tx.commit()?;
        Ok(packed)
    }

    pub fn pack_site(&self, name: &str, format: ArchiveFormat) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let db = self.inner.db.lock().unwrap();
        let archive = site_files(&db, name)?;
        match format {
            ArchiveFormat::Tar => pack_tar(&archive.files),
            ArchiveFormat::TarGz => pack_tar_gz(&archive.files),
            ArchiveFormat::Zip => pack_zip(&archive.files),
        }
        .map_err(StoreError::Io)
    }

    pub fn delete_file(&self, name: &str, rel: &str) -> Result<(), StoreError> {
        let name = parse_site_name(name)?;
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let prefix = format!("{rel}/%");
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        let site_id: i64 = tx
            .query_row(
                "SELECT id FROM sites WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        let deleted = tx.execute(
            "DELETE FROM files WHERE site_id = ?1 AND (path = ?2 OR path LIKE ?3)",
            params![site_id, rel, prefix],
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
        gc_blobs(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn commit_site(&self, name: &str, files: &[StagedFile]) -> Result<(), StoreError> {
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        for file in files {
            tx.execute(
                "INSERT OR IGNORE INTO blobs (hash, bytes, size) VALUES (?1, ?2, ?3)",
                params![file.hash, file.bytes, file.size],
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
        gc_blobs(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn upsert_file(&self, name: &str, file: &StagedFile) -> Result<(), StoreError> {
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, bytes, size) VALUES (?1, ?2, ?3)",
            params![file.hash, file.bytes, file.size],
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
        gc_blobs(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn migrate_legacy(&self) -> Result<(), StoreError> {
        {
            let db = self.inner.db.lock().unwrap();
            let n: i64 = db.query_row("SELECT COUNT(*) FROM sites", [], |row| row.get(0))?;
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
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut apple = HashSet::new();
        {
            let mut stmt = tx.prepare(
                "SELECT hash, CAST(substr(bytes, 1, 4) AS BLOB) FROM blobs WHERE size <= 65536",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in rows {
                let (hash, prefix) = row?;
                if looks_like_apple_fork(&prefix) {
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
        gc_blobs(&tx)?;
        tx.commit()?;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Missing,
    Dir,
    File,
}

fn site_exists_locked(db: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
    db.query_row("SELECT 1 FROM sites WHERE name = ?1", params![name], |_| {
        Ok(())
    })
    .optional()
    .map(|row| row.is_some())
}

fn node_locked(db: &Connection, name: &str, rel: &str) -> Result<NodeKind, StoreError> {
    if !site_exists_locked(db, name)? {
        return Ok(NodeKind::Missing);
    }
    if rel.is_empty() {
        return Ok(NodeKind::Dir);
    }
    let kind: i64 = db.query_row(
        "SELECT CASE
            WHEN EXISTS (
                SELECT 1 FROM files
                WHERE site_id = (SELECT id FROM sites WHERE name = ?1) AND path = ?2
            ) THEN 1
            WHEN EXISTS (
                SELECT 1 FROM files
                WHERE site_id = (SELECT id FROM sites WHERE name = ?1) AND path LIKE ?3
            ) THEN 2
            ELSE 0
         END",
        params![name, rel, format!("{rel}/%")],
        |row| row.get(0),
    )?;
    Ok(match kind {
        1 => NodeKind::File,
        2 => NodeKind::Dir,
        _ => NodeKind::Missing,
    })
}

fn load_sizes(db: &Connection, sql: &str) -> Result<Vec<u64>, rusqlite::Error> {
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0).map(|size| size as u64))?;
    rows.collect()
}

fn quantile(sorted: &[u64], p: f64) -> f64 {
    let position = (sorted.len() - 1) as f64 * p;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let weight = position - lower as f64;
    (sorted[upper] as f64).mul_add(weight, sorted[lower] as f64 * (1.0 - weight))
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
    let p25 = quantile(sorted, 0.25);
    let median = quantile(sorted, 0.5);
    let p75 = quantile(sorted, 0.75);
    let mean = sorted.iter().map(|size| *size as f64).sum::<f64>() / sorted.len() as f64;
    let variance = sorted
        .iter()
        .map(|size| {
            let delta = *size as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / sorted.len() as f64;
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
            let bytes = fs::read(entry.path())?;
            if is_junk(Path::new(&rel), Some(&bytes)) {
                continue;
            }
            out.push(stage_bytes(&rel, &bytes));
        }
    }
    Ok(())
}

fn stage_bytes(path: &str, bytes: &[u8]) -> StagedFile {
    let hash = blake3::hash(bytes);
    StagedFile {
        path: path.to_string(),
        size: bytes.len() as i64,
        hash: hash.to_hex().to_string(),
        bytes: bytes.to_vec(),
    }
}

fn site_files(db: &Connection, name: &str) -> Result<SiteArchive, StoreError> {
    let site_id: i64 = db
        .query_row(
            "SELECT id FROM sites WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    let mut stmt = db.prepare(
        "SELECT files.path, blobs.bytes
         FROM files JOIN blobs ON blobs.hash = files.hash
         WHERE files.site_id = ?1
         ORDER BY files.path",
    )?;
    let rows = stmt.query_map(params![site_id], |row| {
        Ok(ArchiveFile {
            path: row.get(0)?,
            bytes: row.get(1)?,
        })
    })?;
    let files = rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|file| !is_junk(Path::new(&file.path), Some(&file.bytes)))
        .collect();
    Ok(SiteArchive { files })
}

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

fn pack_tar(files: &[ArchiveFile]) -> io::Result<Vec<u8>> {
    append_tar(Vec::new(), files)
}

fn pack_tar_gz(files: &[ArchiveFile]) -> io::Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    append_tar(encoder, files)?.finish()
}

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

fn gc_blobs(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute(
        "DELETE FROM blobs WHERE hash NOT IN (SELECT DISTINCT hash FROM files)",
        [],
    )?;
    Ok(())
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
        .map_or(0, |d| d.as_millis() as i64)
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
        assert_eq!(store.read_blob(&hash).unwrap(), b"<h1>x</h1>");
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
            let hash = blake3::hash(&apple).to_hex().to_string();
            db.execute(
                "INSERT INTO blobs (hash, bytes, size) VALUES (?1, ?2, ?3)",
                params![hash, apple.as_slice(), apple.len() as i64],
            )
            .unwrap();
            let site_id: i64 = db
                .query_row("SELECT id FROM sites WHERE name = 'hello'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            db.execute(
                "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)",
                params![site_id, "._index.html", hash, apple.len() as i64],
            )
            .unwrap();
            db.execute(
                "INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)",
                params![site_id, "keep.bin", hash, apple.len() as i64],
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
        assert_eq!(stats.saved_fraction, 0.5);
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
}
