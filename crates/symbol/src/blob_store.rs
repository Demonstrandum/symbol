use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash::ContentHash;

pub struct BlobFiles {
    root: PathBuf,
}

impl BlobFiles {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self, hash: ContentHash) -> PathBuf {
        let (prefix, rest) = split_hex(hash);
        self.root.join(prefix).join(rest)
    }

    pub fn read(&self, hash: ContentHash) -> io::Result<Vec<u8>> {
        fs::read(self.path(hash))
    }

    pub fn put_bytes(&self, hash: ContentHash, bytes: &[u8]) -> io::Result<()> {
        self.put(hash, |file| file.write_all(bytes))
    }

    pub fn put_file(&self, hash: ContentHash, source: &Path) -> io::Result<()> {
        self.put(hash, |file| {
            let mut source = File::open(source)?;
            io::copy(&mut source, file)?;
            Ok(())
        })
    }

    pub fn quarantine(&self, hash: ContentHash) -> io::Result<()> {
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

    pub fn restore(&self, live: &HashSet<ContentHash>) -> io::Result<()> {
        for &hash in live {
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

    fn quarantine_path(&self, hash: ContentHash) -> PathBuf {
        let (prefix, rest) = split_hex(hash);
        self.root.join(".quarantine").join(prefix).join(rest)
    }

    fn put(
        &self,
        hash: ContentHash,
        write: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<()> {
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
        let temporary = parent.join(format!(
            ".{}-{}-{nonce}.tmp",
            hash.to_hex(),
            std::process::id()
        ));
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

    fn quarantine_corrupt(&self, hash: ContentHash, source: &Path) -> io::Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let target = self
            .root
            .join(".quarantine")
            .join("corrupt")
            .join(format!("{}-{nonce}", hash.to_hex()));
        let parent = target.parent().expect("corrupt quarantine path has parent");
        fs::create_dir_all(parent)?;
        fs::rename(source, &target)?;
        File::open(parent)?.sync_all()
    }
}

/// Split a hash into the two-character directory prefix and the remainder.
fn split_hex(hash: ContentHash) -> (String, String) {
    let hex = hash.to_hex();
    let (prefix, rest) = hex.split_at(2);
    (prefix.to_owned(), rest.to_owned())
}

fn file_hash(path: &Path) -> io::Result<ContentHash> {
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
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_corrupt_canonical_blob_preserves_the_old_bytes() {
        let root = tempfile::tempdir().unwrap();
        let blobs = BlobFiles::new(root.path().join("blobs")).unwrap();
        let expected = b"expected";
        let hash = ContentHash::from(blake3::hash(expected));
        let target = blobs.path(hash);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"corrupt").unwrap();

        blobs.put_bytes(hash, expected).unwrap();

        assert_eq!(fs::read(target).unwrap(), expected);
        let corrupt = root.path().join("blobs/.quarantine/corrupt");
        let preserved = fs::read_dir(corrupt)
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(preserved, vec![b"corrupt".to_vec()]);
    }
}
