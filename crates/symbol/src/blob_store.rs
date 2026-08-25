use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct BlobFiles {
    root: PathBuf,
}

impl BlobFiles {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[..2]).join(&hash[2..])
    }

    pub fn read(&self, hash: &str) -> io::Result<Vec<u8>> {
        fs::read(self.path(hash))
    }

    pub fn put_bytes(&self, hash: &str, bytes: &[u8]) -> io::Result<()> {
        self.put(hash, |file| file.write_all(bytes))
    }

    pub fn put_file(&self, hash: &str, source: &Path) -> io::Result<()> {
        self.put(hash, |file| {
            let mut source = File::open(source)?;
            io::copy(&mut source, file)?;
            Ok(())
        })
    }

    pub fn remove(&self, hash: &str) -> io::Result<()> {
        match fs::remove_file(self.path(hash)) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub fn retain(&self, live: &HashSet<String>) -> io::Result<()> {
        for directory in fs::read_dir(&self.root)? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            let prefix = directory.file_name().to_string_lossy().into_owned();
            for entry in fs::read_dir(directory.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let hash = format!("{prefix}{name}");
                if name.starts_with('.') || !live.contains(&hash) {
                    fs::remove_file(entry.path())?;
                }
            }
            if fs::read_dir(directory.path())?.next().is_none() {
                fs::remove_dir(directory.path())?;
            }
        }
        Ok(())
    }

    fn put(&self, hash: &str, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
        let target = self.path(hash);
        if target.is_file() {
            if file_hash(&target)? == hash {
                return Ok(());
            }
            fs::remove_file(&target)?;
        }
        let parent = target.parent().expect("blob path has parent");
        fs::create_dir_all(parent)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let temporary = parent.join(format!(".{hash}-{}-{nonce}.tmp", std::process::id()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            write(&mut file)?;
            file.sync_all()?;
            fs::rename(&temporary, &target)?;
            File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

fn file_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
