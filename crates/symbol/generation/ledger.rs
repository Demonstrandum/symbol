use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

use super::{
    BumpIntent, GenerationError, GenerationMode, INITIAL_API_VERSION, UPDATE_COMMAND,
    canonical_source_hash,
};

const LEDGER_NAME: &str = "api-version.toml";
const LOCK_NAME: &str = ".api-version.lock";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl ApiVersion {
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    #[must_use]
    /// Advances the patch component.
    ///
    /// # Panics
    ///
    /// Panics if the patch component is already `u64::MAX`.
    pub const fn next_patch(self) -> Self {
        Self {
            patch: self
                .patch
                .checked_add(1)
                .expect("API patch version overflow"),
            ..self
        }
    }

    #[must_use]
    /// Advances the minor component and resets the patch component.
    ///
    /// # Panics
    ///
    /// Panics if the minor component is already `u64::MAX`.
    pub const fn next_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self
                .minor
                .checked_add(1)
                .expect("API minor version overflow"),
            patch: 0,
        }
    }

    #[must_use]
    /// Advances the major component and resets lower components.
    ///
    /// # Panics
    ///
    /// Panics if the major component is already `u64::MAX`.
    pub const fn next_major(self) -> Self {
        Self {
            major: self
                .major
                .checked_add(1)
                .expect("API major version overflow"),
            minor: 0,
            patch: 0,
        }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for ApiVersion {
    type Err = GenerationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut components = value.split('.');
        let major = parse_semantic_component(components.next(), value)?;
        let minor = parse_semantic_component(components.next(), value)?;
        let patch = parse_semantic_component(components.next(), value)?;
        if components.next().is_some() {
            return Err(GenerationError::InvalidVersion(value.to_string()));
        }
        Ok(Self::new(major, minor, patch))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHash(String);

impl SourceHash {
    /// Validates and wraps a lowercase 64-character Blake3 digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the digest has the wrong length or alphabet.
    pub fn new(value: String) -> Result<Self, GenerationError> {
        let valid = value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid {
            return Err(GenerationError::InvalidSourceHash(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionLedger {
    pub version: ApiVersion,
    pub absolute_revision: u64,
    pub source_hash: SourceHash,
}

impl VersionLedger {
    #[must_use]
    pub fn to_toml(&self) -> String {
        format!(
            "version = \"{}\"\nabsolute_revision = {}\nsource_hash = \"{}\"\n",
            self.version, self.absolute_revision, self.source_hash
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLedger {
    version: String,
    absolute_revision: u64,
    source_hash: String,
}

/// Reads and validates `api-version.toml`.
///
/// # Errors
///
/// Returns an error if the ledger cannot be read or is malformed.
pub fn read_ledger(root: &Path) -> Result<VersionLedger, GenerationError> {
    let path = root.join(LEDGER_NAME);
    let source = fs::read_to_string(&path).map_err(|source| GenerationError::FileIo {
        path: path.clone(),
        source,
    })?;
    parse_ledger(&source, &path)
}

/// Computes the ledger a writable build would persist without changing files.
///
/// # Errors
///
/// Returns an error if canonical inputs or an existing ledger cannot be read.
///
/// # Panics
///
/// Panics if a stale ledger already contains the maximum patch or absolute revision.
pub fn preview_ledger(root: &Path) -> Result<VersionLedger, GenerationError> {
    let source_hash = canonical_source_hash(root)?;
    let path = root.join(LEDGER_NAME);
    if !path.exists() {
        return Ok(VersionLedger {
            version: INITIAL_API_VERSION,
            absolute_revision: 1,
            source_hash,
        });
    }
    let current = read_ledger(root)?;
    if current.source_hash == source_hash {
        return Ok(current);
    }
    Ok(VersionLedger {
        version: current.version.next_patch(),
        absolute_revision: current
            .absolute_revision
            .checked_add(1)
            .expect("absolute API revision overflow"),
        source_hash,
    })
}

pub fn reconcile_ledger(
    root: &Path,
    mode: GenerationMode,
    intent: BumpIntent,
) -> Result<VersionLedger, GenerationError> {
    match mode {
        GenerationMode::Update => update_ledger(root, intent),
        GenerationMode::ReadOnly if matches!(intent, BumpIntent::Automatic) => check_ledger(root),
        GenerationMode::ReadOnly => Err(GenerationError::BumpRequiresUpdateMode),
    }
}

pub fn lock_workspace(root: &Path) -> Result<WorkspaceLock, GenerationError> {
    WorkspaceLock::acquire(root)
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), GenerationError> {
    let parent = path
        .parent()
        .ok_or_else(|| GenerationError::InvalidOutputPath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| GenerationError::FileIo {
        path: parent.to_path_buf(),
        source,
    })?;
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .ok_or_else(|| GenerationError::InvalidOutputPath(path.to_path_buf()))?
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        sequence
    ));
    let result = write_and_replace(path, &temp, contents);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn update_ledger(root: &Path, intent: BumpIntent) -> Result<VersionLedger, GenerationError> {
    let _lock = WorkspaceLock::acquire(root)?;
    let source_hash = canonical_source_hash(root)?;
    let path = root.join(LEDGER_NAME);
    let next = match intent {
        BumpIntent::Automatic => update_ledger_automatically(root, &path, source_hash)?,
        BumpIntent::Explicit(request) => apply_explicit_bump(root, &path, source_hash, &request)?,
    };
    if path.exists() && read_ledger(root)? == next {
        return Ok(next);
    }
    write_atomic(&path, next.to_toml().as_bytes())?;
    Ok(next)
}

fn update_ledger_automatically(
    root: &Path,
    path: &Path,
    source_hash: SourceHash,
) -> Result<VersionLedger, GenerationError> {
    if !path.exists() {
        return Ok(VersionLedger {
            version: INITIAL_API_VERSION,
            absolute_revision: 1,
            source_hash,
        });
    }
    let current = read_ledger(root)?;
    if current.source_hash == source_hash {
        return Ok(current);
    }
    Ok(VersionLedger {
        version: current.version.next_patch(),
        absolute_revision: next_absolute_revision(current.absolute_revision),
        source_hash,
    })
}

fn apply_explicit_bump(
    root: &Path,
    path: &Path,
    source_hash: SourceHash,
    request: &super::ExplicitBumpRequest,
) -> Result<VersionLedger, GenerationError> {
    let expected_target = match request.kind {
        super::BumpKind::Minor => request.expected.version.next_minor(),
        super::BumpKind::Major => request.expected.version.next_major(),
    };
    if request.target != expected_target {
        return Err(GenerationError::InvalidBumpTarget);
    }
    if !path.exists() {
        return Err(bump_conflict(&request.expected, None));
    }
    let current = read_ledger(root)?;
    let applied = VersionLedger {
        version: request.target,
        absolute_revision: next_absolute_revision(request.expected.absolute_revision),
        source_hash,
    };
    if current == applied {
        return Ok(current);
    }
    if current != request.expected {
        return Err(bump_conflict(&request.expected, Some(&current)));
    }
    // The wrapper computes the target before Cargo can auto-reconcile. Thus a
    // dirty source and its semantic bump become this one atomic revision.
    Ok(applied)
}

fn bump_conflict(expected: &VersionLedger, observed: Option<&VersionLedger>) -> GenerationError {
    GenerationError::BumpConflict {
        expected: ledger_identity(expected),
        observed: observed.map_or_else(|| "missing ledger".to_string(), ledger_identity),
    }
}

fn ledger_identity(ledger: &VersionLedger) -> String {
    format!(
        "{}:{}:{}",
        ledger.version, ledger.absolute_revision, ledger.source_hash
    )
}

const fn next_absolute_revision(current: u64) -> u64 {
    current
        .checked_add(1)
        .expect("absolute API revision overflow")
}

fn check_ledger(root: &Path) -> Result<VersionLedger, GenerationError> {
    let ledger = read_ledger(root)?;
    let actual_hash = canonical_source_hash(root)?;
    if ledger.source_hash != actual_hash {
        return Err(GenerationError::StaleLedger {
            recorded: ledger.source_hash,
            actual: actual_hash,
            update_command: UPDATE_COMMAND,
        });
    }
    Ok(ledger)
}

fn parse_ledger(source: &str, path: &Path) -> Result<VersionLedger, GenerationError> {
    let raw: RawLedger = toml::from_str(source).map_err(|source| GenerationError::LedgerToml {
        path: path.to_path_buf(),
        source,
    })?;
    if raw.absolute_revision == 0 {
        return Err(GenerationError::InvalidAbsoluteRevision);
    }
    Ok(VersionLedger {
        version: raw.version.parse()?,
        absolute_revision: raw.absolute_revision,
        source_hash: SourceHash::new(raw.source_hash)?,
    })
}

fn parse_semantic_component(component: Option<&str>, whole: &str) -> Result<u64, GenerationError> {
    let component = component.ok_or_else(|| GenerationError::InvalidVersion(whole.to_string()))?;
    if component.is_empty()
        || (component.len() > 1 && component.starts_with('0'))
        || !component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(GenerationError::InvalidVersion(whole.to_string()));
    }
    component
        .parse()
        .map_err(|_| GenerationError::InvalidVersion(whole.to_string()))
}

fn write_and_replace(path: &Path, temp: &Path, contents: &[u8]) -> Result<(), GenerationError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp)
        .map_err(|source| GenerationError::FileIo {
            path: temp.to_path_buf(),
            source,
        })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| GenerationError::FileIo {
            path: temp.to_path_buf(),
            source,
        })?;
    fs::rename(temp, path).map_err(|source| GenerationError::FileIo {
        path: path.to_path_buf(),
        source,
    })?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<(), GenerationError> {
    let parent = path
        .parent()
        .ok_or_else(|| GenerationError::InvalidOutputPath(path.to_path_buf()))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| GenerationError::FileIo {
            path: parent.to_path_buf(),
            source,
        })
}

pub struct WorkspaceLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl WorkspaceLock {
    fn acquire(root: &Path) -> Result<Self, GenerationError> {
        let path = root.join(LOCK_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| GenerationError::FileIo {
                path: path.clone(),
                source,
            })?;
        file.lock().map_err(|source| GenerationError::FileIo {
            path: path.clone(),
            source,
        })?;
        Ok(Self { file, path })
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
