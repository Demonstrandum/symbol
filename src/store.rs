use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};

use crate::name::{generate_id, parse_site_name, NameError};
use crate::pathutil::{safe_rel_path, PathError};
use crate::upload::{write_payload, Kind, UploadError};

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

#[derive(Debug, Clone)]
pub struct DirEnt {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub sites: i64,
    pub files: i64,
    pub blobs: i64,
    pub bytes: i64,
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
                root: root.clone(),
                db: Mutex::new(db),
            }),
        };
        store.migrate_legacy()?;
        Ok(store)
    }

    pub fn stats(&self) -> Result<Stats, StoreError> {
        let db = self.inner.db.lock().unwrap();
        let sites: i64 = db.query_row("SELECT COUNT(*) FROM sites", [], |row| row.get(0))?;
        let files: i64 = db.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        let blobs: i64 = db.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
        let bytes: i64 =
            db.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |row| row.get(0))?;
        Ok(Stats {
            sites,
            files,
            blobs,
            bytes,
        })
    }

    pub fn list_sites(&self) -> Result<Vec<String>, StoreError> {
        let db = self.inner.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT name FROM sites ORDER BY name")?;
        let names = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(names)
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

    pub fn list_dir(&self, name: &str, rel: &str) -> Result<Vec<DirEnt>, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
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
                Ok(Node::File {
                    logical: rel,
                    hash,
                })
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
        db.query_row("SELECT bytes FROM blobs WHERE hash = ?1", [hash], |row| row.get(0))
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
        let n = match write_payload(&tmp, bytes, kind, filename, unpack) {
            Ok(n) => n,
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        };
        let staged = match stage_dir(&tmp) {
            Ok(files) => files,
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        };
        let _ = fs::remove_dir_all(&tmp);
        self.commit_site(&name, &staged)?;
        Ok(n)
    }

    pub fn put_file(&self, name: &str, rel: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let staged = stage_bytes(&rel, bytes);
        self.upsert_file(&name, &staged)
    }

    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        let site_id: i64 = tx
            .query_row("SELECT id FROM sites WHERE name = ?1", params![name], |row| row.get(0))
            .map_err(map_sql)?;
        let files = {
            let mut stmt = tx.prepare(
                "SELECT files.path, blobs.bytes
                 FROM files JOIN blobs ON blobs.hash = files.hash
                 WHERE files.site_id = ?1
                 ORDER BY files.path",
            )?;
            let rows = stmt.query_map(params![site_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            let mut files = Vec::new();
            for row in rows {
                files.push(row?);
            }
            files
        };
        let packed = pack_tar_gz(&files)?;
        tx.execute("DELETE FROM sites WHERE name = ?1", params![name])?;
        gc_blobs(&tx)?;
        tx.commit()?;
        Ok(packed)
    }

    pub fn delete_file(&self, name: &str, rel: &str) -> Result<(), StoreError> {
        let name = parse_site_name(name)?;
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let prefix = format!("{rel}/%");
        let mut db = self.inner.db.lock().unwrap();
        let tx = db.transaction()?;
        let site_id: i64 = tx
            .query_row("SELECT id FROM sites WHERE name = ?1", params![name], |row| row.get(0))
            .map_err(map_sql)?;
        let deleted = tx.execute(
            "DELETE FROM files WHERE site_id = ?1 AND (path = ?2 OR path LIKE ?3)",
            params![site_id, rel, prefix],
        )?;
        if deleted == 0 {
            return Err(StoreError::NotFound);
        }
        let remaining: i64 =
            tx.query_row("SELECT COUNT(*) FROM files WHERE site_id = ?1", params![site_id], |row| {
                row.get(0)
            })?;
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
        let site_id: i64 = tx.query_row("SELECT id FROM sites WHERE name = ?1", params![name], |row| row.get(0))?;
        tx.execute("DELETE FROM files WHERE site_id = ?1", params![site_id])?;
        {
            let mut ins = tx.prepare("INSERT INTO files (site_id, path, hash, size) VALUES (?1, ?2, ?3, ?4)")?;
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
        let site_id: i64 = tx.query_row("SELECT id FROM sites WHERE name = ?1", params![name], |row| row.get(0))?;
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
                let blob = self.inner.root.join("blobs").join(&file.hash[..2]).join(&file.hash[2..]);
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

    fn tmp_dir(&self, name: &str) -> PathBuf {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
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
    db.query_row("SELECT 1 FROM sites WHERE name = ?1", params![name], |_| Ok(()))
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

fn dirents(files: &[(String, u64)], rel: &str) -> Vec<DirEnt> {
    let prefix = if rel.is_empty() {
        String::new()
    } else {
        format!("{rel}/")
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut out: Vec<DirEnt> = Vec::new();
    for (path, size) in files {
        let rest = if prefix.is_empty() {
            path.as_str()
        } else if let Some(r) = path.strip_prefix(&prefix) {
            r
        } else {
            continue;
        };
        if let Some((dir, _)) = rest.split_once('/') {
            if !dirs.iter().any(|d| d == dir) {
                dirs.push(dir.to_string());
            }
        } else {
            out.push(DirEnt {
                name: rest.to_string(),
                is_dir: false,
                size: *size,
            });
        }
    }
    out.extend(dirs.into_iter().map(|name| DirEnt {
        name,
        is_dir: true,
        size: 0,
    }));
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    out
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

fn pack_tar_gz(files: &[(String, Vec<u8>)]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let encoder = GzEncoder::new(&mut out, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, path, bytes.as_slice())?;
        }
        archive.into_inner()?.finish()?;
    }
    Ok(out)
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
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        assert_eq!(store.list_sites().unwrap(), vec!["hello".to_string()]);
        assert_eq!(store.list_files("hello").unwrap(), vec!["index.html".to_string()]);
        assert!(dir.path().join("symbol.db").is_file());
        let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
            panic!("expected file");
        };
        assert_eq!(store.read_blob(&hash).unwrap(), b"<h1>x</h1>");
        let packed = store.pop_site("hello").unwrap();
        assert_eq!(&packed[..2], [0x1f, 0x8b]);
        assert!(store.list_sites().unwrap().is_empty());
        assert!(matches!(
            store.read_blob(&hash).unwrap_err(),
            StoreError::NotFound
        ));
    }
}
