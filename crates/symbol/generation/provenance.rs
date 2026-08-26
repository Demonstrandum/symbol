use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::GenerationError;

pub const COMMIT_ENV: &str = "SYMBOL_BUILD_COMMIT";
pub const DIRTY_ENV: &str = "SYMBOL_BUILD_DIRTY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProvenance {
    pub commit: String,
    pub dirty: bool,
}

impl BuildProvenance {
    /// Creates validated provenance from an injected commit and dirty bit.
    ///
    /// # Errors
    ///
    /// Returns an error unless the commit is raw hexadecimal or `unknown`.
    pub fn from_injected(commit: String, dirty: bool) -> Result<Self, GenerationError> {
        let provenance = Self { commit, dirty };
        provenance.validate()?;
        Ok(provenance)
    }

    /// Resolves injected provenance or reads the local Git worktree without fetching.
    ///
    /// Local dirty state matches Git/Nix tracked-state semantics: staged or
    /// unstaged changes to any tracked file are dirty, while untracked files
    /// are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error when injected values are incomplete or Git cannot report
    /// the local commit and worktree state.
    pub fn discover(root: &Path) -> Result<Self, GenerationError> {
        let commit = env::var_os(COMMIT_ENV);
        let dirty = env::var_os(DIRTY_ENV);
        match (commit, dirty) {
            (None, None) => discover_git(root),
            (Some(commit), Some(dirty)) => {
                let commit = commit
                    .into_string()
                    .map_err(|_| GenerationError::InvalidProvenanceEnvironment(COMMIT_ENV))?;
                let dirty = dirty
                    .into_string()
                    .map_err(|_| GenerationError::InvalidProvenanceEnvironment(DIRTY_ENV))?;
                injected_provenance(commit, &dirty)
            }
            _ => Err(GenerationError::IncompleteProvenanceEnvironment),
        }
    }

    /// Validates provenance before it is embedded in any generated language.
    ///
    /// # Errors
    ///
    /// Returns an error unless the commit is raw hexadecimal or `unknown`.
    pub fn validate(&self) -> Result<(), GenerationError> {
        let valid = self.commit == "unknown"
            || ((7..=64).contains(&self.commit.len())
                && self.commit.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if !valid {
            return Err(GenerationError::InvalidGitCommit);
        }
        Ok(())
    }
}

/// Lists tracked Git files so build scripts can rerun when dirty state changes.
///
/// # Errors
///
/// Returns an error when the local Git command cannot run or fails.
pub fn tracked_git_files(root: &Path) -> Result<Vec<PathBuf>, GenerationError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|source| GenerationError::GitIo { source })?;
    if !output.status.success() {
        return Err(GenerationError::GitCommand {
            command: "git ls-files",
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output
        .stdout
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
        .map(|path| root.join(String::from_utf8_lossy(path).as_ref()))
        .collect())
}

/// Returns Git metadata paths that can change local commit or dirty provenance.
///
/// # Errors
///
/// Returns an error when Git cannot resolve its metadata paths.
pub fn git_watch_paths(root: &Path) -> Result<Vec<PathBuf>, GenerationError> {
    let mut paths = vec![
        resolve_git_path(root, "HEAD")?,
        resolve_git_path(root, "index")?,
    ];
    let symbolic_ref = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .map_err(|source| GenerationError::GitIo { source })?;
    if symbolic_ref.status.success() {
        let reference = String::from_utf8_lossy(&symbolic_ref.stdout);
        paths.push(resolve_git_path(root, reference.trim())?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[derive(Clone, Copy)]
enum GitQuery {
    Head,
    Dirty,
}

fn resolve_with_probe(
    mut probe: impl FnMut(GitQuery) -> Result<String, GenerationError>,
) -> Result<BuildProvenance, GenerationError> {
    let commit = probe(GitQuery::Head)?;
    let dirty = !probe(GitQuery::Dirty)?.is_empty();
    BuildProvenance::from_injected(commit, dirty)
}

fn discover_git(root: &Path) -> Result<BuildProvenance, GenerationError> {
    resolve_with_probe(|query| run_git_query(root, query))
}

fn injected_provenance(commit: String, dirty: &str) -> Result<BuildProvenance, GenerationError> {
    let dirty = match dirty {
        "true" => true,
        "false" => false,
        _ => return Err(GenerationError::InvalidDirtyValue(dirty.to_string())),
    };
    BuildProvenance::from_injected(commit, dirty)
}

fn resolve_git_path(root: &Path, name: &str) -> Result<PathBuf, GenerationError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-path", name])
        .output()
        .map_err(|source| GenerationError::GitIo { source })?;
    if !output.status.success() {
        return Err(GenerationError::GitCommand {
            command: "git rev-parse --git-path",
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn run_git_query(root: &Path, query: GitQuery) -> Result<String, GenerationError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root);
    match query {
        GitQuery::Head => {
            command.args(["rev-parse", "HEAD"]);
        }
        GitQuery::Dirty => {
            command.args(["status", "--porcelain=v1", "--untracked-files=no"]);
        }
    }
    let output = command
        .output()
        .map_err(|source| GenerationError::GitIo { source })?;
    if !output.status.success() {
        return Err(GenerationError::GitCommand {
            command: match query {
                GitQuery::Head => "git rev-parse HEAD",
                GitQuery::Dirty => "git status --porcelain=v1 --untracked-files=no",
            },
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
#[path = "provenance_tests.rs"]
mod tests;
