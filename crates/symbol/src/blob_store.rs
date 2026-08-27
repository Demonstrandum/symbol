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

    pub fn quarantine(&self, hash: &str) -> io::Result<()> {
        let source = self.path(hash);
        if !source.is_file() {
            return Ok(());
        }
        let target = self.quarantine_path(hash);
        if target.is_file() {
            return Ok(());
        }
        let parent = target.parent().expect("quarantine blob path has parent");
        fs::create_dir_all(parent)?;
        fs::rename(source, &target)?;
        File::open(parent)?.sync_all()
    }

    pub fn restore(&self, live: &HashSet<String>) -> io::Result<()> {
        for hash in live {
            let target = self.path(hash);
            if target.is_file() {
                continue;
            }
            let source = self.quarantine_path(hash);
            if !source.is_file() {
                continue;
            }
            let parent = target.parent().expect("blob path has parent");
            fs::create_dir_all(parent)?;
            fs::rename(source, &target)?;
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    fn quarantine_path(&self, hash: &str) -> PathBuf {
        self.root
            .join(".quarantine")
            .join(&hash[..2])
            .join(&hash[2..])
    }

    fn put(&self, hash: &str, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
        let target = self.path(hash);
        if !target.is_file() {
            let quarantined = self.quarantine_path(hash);
            if quarantined.is_file() {
                let parent = target.parent().expect("blob path has parent");
                fs::create_dir_all(parent)?;
                fs::rename(quarantined, &target)?;
            }
        }
        if target.is_file() {
            if file_hash(&target)? == hash {
                return Ok(());
            }
            self.quarantine_corrupt(hash, &target)?;
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

    fn quarantine_corrupt(&self, hash: &str, source: &Path) -> io::Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let target = self
            .root
            .join(".quarantine")
            .join("corrupt")
            .join(format!("{hash}-{nonce}"));
        let parent = target.parent().expect("corrupt quarantine path has parent");
        fs::create_dir_all(parent)?;
        fs::rename(source, &target)?;
        File::open(parent)?.sync_all()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_corrupt_canonical_blob_preserves_the_old_bytes() {
        let root = tempfile::tempdir().unwrap();
        let blobs = BlobFiles::new(root.path().join("blobs")).unwrap();
        let expected = b"expected";
        let hash = blake3::hash(expected).to_hex().to_string();
        let target = blobs.path(&hash);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"corrupt").unwrap();

        blobs.put_bytes(&hash, expected).unwrap();

        assert_eq!(fs::read(target).unwrap(), expected);
        let corrupt = root.path().join("blobs/.quarantine/corrupt");
        let preserved = fs::read_dir(corrupt)
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(preserved, vec![b"corrupt".to_vec()]);
    }
}
