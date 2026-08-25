use std::collections::{BTreeMap, HashSet};
#[cfg(test)]
use std::io::Cursor;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::Path;
use std::sync::OnceLock;

use flate2::read::GzDecoder;

#[cfg(test)]
use crate::pathutil::is_junk;
use crate::pathutil::{PathError, is_noise_path, looks_like_apple_fork, safe_rel_path};
use crate::sanitize;

const DEFAULT_MAX_FILES: usize = 5000;
const DEFAULT_MAX_EXTRACTED: u64 = 80 * 1024 * 1024;
static ARCHIVE_LIMITS: OnceLock<ArchiveLimits> = OnceLock::new();
pub const MAX_ALIAS_TARGET_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub max_files: usize,
    pub max_extracted: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_extracted: DEFAULT_MAX_EXTRACTED,
        }
    }
}

pub fn configure_archive_limits(limits: ArchiveLimits) -> Result<(), ArchiveLimits> {
    ARCHIVE_LIMITS.set(limits)
}

fn archive_limits() -> ArchiveLimits {
    ARCHIVE_LIMITS.get().copied().unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("error: empty body")]
    Empty,
    #[error("error: archive has no files")]
    EmptyArchive,
    #[error("error: not an archive (pass a zip, tar, tar.gz, or gz, or drop Unpack)")]
    NotArchive,
    #[error("error: junk file")]
    Junk,
    #[error("error: too many files")]
    TooManyFiles,
    #[error("error: archive upload is too large")]
    ArchiveTooLarge,
    #[error("error: file upload is too large")]
    FileTooLarge,
    #[error("error: extracted site is too large")]
    TooLarge,
    #[error("error: path is reserved by symbol")]
    ReservedPath,
    #[error("error: supported archive contains a Symbol management secret; unpack or remove it")]
    OpaqueSecret,
    #[allow(dead_code)]
    #[error("error: archive alias target is invalid")]
    InvalidAlias,
    #[allow(dead_code)]
    #[error("error: archive alias cycle detected")]
    AliasCycle,
    #[error("{0}")]
    Path(#[from] PathError),
    #[error("error: zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ArchiveMember {
    File {
        path: String,
    },
    Alias {
        path: String,
        canonical_target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ArchivePlan {
    pub members: Vec<ArchiveMember>,
}

#[allow(dead_code)]
pub fn plan_archive(source: &Path, kind: Kind) -> Result<ArchivePlan, UploadError> {
    let members = match kind {
        Kind::Zip => plan_zip(std::fs::File::open(source)?)?,
        Kind::Tar => plan_tar(std::fs::File::open(source)?)?,
        Kind::Gzip => {
            let decoder = GzDecoder::new(std::fs::File::open(source)?);
            plan_tar(decoder)?
        }
        Kind::Html | Kind::File => return Err(UploadError::NotArchive),
    };
    validate_archive_members(members)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Tar,
    Gzip,
    Html,
    File,
}

impl Kind {
    pub const fn default_filename(self) -> &'static str {
        match self {
            Self::Zip => "archive.zip",
            Self::Tar => "archive.tar",
            Self::Gzip => "archive.gz",
            Self::Html => "index.html",
            Self::File => "file",
        }
    }
}

pub fn sniff(bytes: &[u8], content_type: Option<&str>, filename: Option<&str>) -> Kind {
    if bytes.len() >= 4
        && bytes[0] == b'P'
        && bytes[1] == b'K'
        && bytes[2] == 0x03
        && bytes[3] == 0x04
    {
        return Kind::Zip;
    }
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return Kind::Gzip;
    }
    if looks_like_tar(bytes) {
        return Kind::Tar;
    }
    if let Some(ct) = content_type {
        let ct = ct.split(';').next().unwrap_or(ct).trim();
        match ct {
            "application/zip" | "application/x-zip-compressed" => return Kind::Zip,
            "application/x-tar" | "application/tar" => return Kind::Tar,
            "application/gzip" | "application/x-gzip" | "application/x-gtar" => return Kind::Gzip,
            "text/html" => return Kind::Html,
            _ => {}
        }
    }
    if let Some(name) = filename {
        if has_extension(name, "zip") {
            return Kind::Zip;
        }
        if has_extension(name, "tgz") || has_extension(name, "tar") || has_extension(name, "gz") {
            return if has_extension(name, "tar") {
                Kind::Tar
            } else {
                Kind::Gzip
            };
        }
        if has_extension(name, "html") || has_extension(name, "htm") {
            return Kind::Html;
        }
    }
    if looks_like_html(bytes) {
        return Kind::Html;
    }
    Kind::File
}

fn looks_like_tar(bytes: &[u8]) -> bool {
    bytes.len() >= 262 && &bytes[257..262] == b"ustar"
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(0);
    let rest = &bytes[start..];
    rest.starts_with(b"<!DOCTYPE")
        || rest.starts_with(b"<!doctype")
        || rest.starts_with(b"<html")
        || rest.starts_with(b"<HTML")
        || rest.starts_with(b"<head")
        || rest.starts_with(b"<HEAD")
}

fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(extension))
}

fn has_ascii_suffix(value: &str, suffix: &str) -> bool {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

#[cfg(test)]
pub fn write_payload(
    dest: &Path,
    bytes: &[u8],
    kind: Kind,
    filename: Option<&str>,
    unpack: bool,
) -> Result<usize, UploadError> {
    if bytes.is_empty() {
        return Err(UploadError::Empty);
    }
    if unpack {
        return unpack_payload(dest, bytes, kind, filename);
    }
    let name = filename
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kind.default_filename());
    let rel = safe_rel_path(name)?;
    if is_junk(&rel, Some(bytes)) {
        return Err(UploadError::Junk);
    }
    let path = dest.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(1)
}

pub fn write_payload_file(
    dest: &Path,
    source: &Path,
    kind: Kind,
    filename: Option<&str>,
    unpack: bool,
) -> Result<usize, UploadError> {
    if !unpack {
        let name = filename
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| kind.default_filename());
        let rel = safe_rel_path(name)?;
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, path)?;
        return Ok(1);
    }
    match kind {
        Kind::Zip => extract_zip_reader(dest, std::fs::File::open(source)?),
        Kind::Tar => extract_tar(dest, std::fs::File::open(source)?),
        Kind::Gzip => extract_gzip_reader(dest, source, filename),
        Kind::Html | Kind::File => Err(UploadError::NotArchive),
    }
}

pub fn reject_secrets_in_opaque_archive(source: &Path, kind: Kind) -> Result<(), UploadError> {
    let found = match kind {
        Kind::Zip => zip_contains_secret(source)?,
        Kind::Tar => tar_contains_secret(std::fs::File::open(source)?)?,
        Kind::Gzip => gzip_contains_secret(source)?,
        Kind::Html | Kind::File => false,
    };
    if found {
        Err(UploadError::OpaqueSecret)
    } else {
        Ok(())
    }
}

fn zip_contains_secret(source: &Path) -> Result<bool, UploadError> {
    let limits = archive_limits();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > limits.max_extracted {
            return Err(UploadError::TooLarge);
        }
        if sanitize::count_reader(&mut entry)?.total() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn tar_contains_secret(reader: impl Read) -> Result<bool, UploadError> {
    let limits = archive_limits();
    let mut archive = tar::Archive::new(reader);
    let mut total = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        total = total.saturating_add(entry.header().size()?);
        if total > limits.max_extracted {
            return Err(UploadError::TooLarge);
        }
        if sanitize::count_reader(&mut entry)?.total() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn gzip_contains_secret(source: &Path) -> Result<bool, UploadError> {
    let limits = archive_limits();
    let decoder = GzDecoder::new(std::fs::File::open(source)?);
    let mut reader = BufReader::new(decoder);
    if looks_like_tar(reader.fill_buf()?) {
        return tar_contains_secret(reader);
    }
    let mut limited = reader.take(limits.max_extracted + 1);
    let found = sanitize::count_reader(&mut limited)?.total() > 0;
    if limited.limit() == 0 {
        return Err(UploadError::TooLarge);
    }
    Ok(found)
}

#[cfg(test)]
fn unpack_payload(
    dest: &Path,
    bytes: &[u8],
    kind: Kind,
    filename: Option<&str>,
) -> Result<usize, UploadError> {
    match kind {
        Kind::Zip => extract_zip(dest, bytes),
        Kind::Tar => extract_tar(dest, Cursor::new(bytes)),
        Kind::Gzip => extract_gzip(dest, bytes, filename),
        Kind::Html | Kind::File => Err(UploadError::NotArchive),
    }
}

#[cfg(test)]
fn extract_gzip(dest: &Path, bytes: &[u8], filename: Option<&str>) -> Result<usize, UploadError> {
    let inner = gunzip(bytes)?;
    let name = filename.unwrap_or("");
    let as_tar = looks_like_tar(&inner)
        || has_ascii_suffix(name, ".tar.gz")
        || has_extension(name, "tgz")
        || has_extension(name, "tar");
    if as_tar {
        return extract_tar(dest, Cursor::new(inner));
    }
    let out_name = strip_gz_name(name).unwrap_or("file");
    let rel = safe_rel_path(out_name)?;
    if is_junk(&rel, Some(&inner)) {
        return Err(UploadError::EmptyArchive);
    }
    let path = dest.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, inner)?;
    Ok(1)
}

fn extract_gzip_reader(
    dest: &Path,
    source: &Path,
    filename: Option<&str>,
) -> Result<usize, UploadError> {
    let limits = archive_limits();
    let decoder = GzDecoder::new(std::fs::File::open(source)?);
    let mut reader = BufReader::new(decoder);
    let prefix = reader.fill_buf()?;
    let name = filename.unwrap_or("");
    let as_tar = looks_like_tar(prefix)
        || has_ascii_suffix(name, ".tar.gz")
        || has_extension(name, "tgz")
        || has_extension(name, "tar");
    if as_tar {
        return extract_tar(dest, reader);
    }
    let out_name = strip_gz_name(name).unwrap_or("file");
    let rel = safe_rel_path(out_name)?;
    let path = dest.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut output = std::fs::File::create(path)?;
    let copied = std::io::copy(&mut reader.take(limits.max_extracted + 1), &mut output)?;
    if copied > limits.max_extracted {
        return Err(UploadError::TooLarge);
    }
    Ok(1)
}

fn strip_gz_name(name: &str) -> Option<&str> {
    if name.is_empty() {
        return None;
    }
    if has_extension(name, "gz") {
        Some(&name[..name.len() - 3])
    } else {
        Some(name)
    }
}

#[allow(dead_code)]
fn plan_zip<R: Read + Seek>(reader: R) -> Result<Vec<ArchiveMember>, UploadError> {
    let limits = archive_limits();
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut members = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let enclosed = entry
            .enclosed_name()
            .ok_or(UploadError::Path(PathError::Invalid))?;
        if is_noise_path(&enclosed) {
            continue;
        }
        let path = safe_rel_path(&enclosed.to_string_lossy())?
            .to_string_lossy()
            .replace('\\', "/");
        let is_symlink = entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000);
        if is_symlink {
            let mut target = String::new();
            let read_limit =
                u64::try_from(MAX_ALIAS_TARGET_BYTES + 1).expect("alias target limit fits in u64");
            if (&mut entry)
                .take(read_limit)
                .read_to_string(&mut target)
                .is_err()
            {
                return Err(UploadError::InvalidAlias);
            }
            if target.len() > MAX_ALIAS_TARGET_BYTES {
                return Err(UploadError::InvalidAlias);
            }
            members.push(ArchiveMember::Alias {
                canonical_target: canonical_archive_target(&path, &target)?,
                path,
            });
        } else if entry.is_file() {
            members.push(ArchiveMember::File { path });
        }
        if members.len() > limits.max_files {
            return Err(UploadError::TooManyFiles);
        }
    }
    Ok(members)
}

#[allow(dead_code)]
fn plan_tar<R: Read>(reader: R) -> Result<Vec<ArchiveMember>, UploadError> {
    let limits = archive_limits();
    let mut archive = tar::Archive::new(reader);
    let mut members = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        let path = path.to_str().ok_or(UploadError::Path(PathError::Invalid))?;
        if is_noise_path(Path::new(path)) {
            continue;
        }
        let path = safe_rel_path(path)?.to_string_lossy().replace('\\', "/");
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() {
            let target = entry
                .link_name()?
                .ok_or(UploadError::InvalidAlias)?
                .to_str()
                .ok_or(UploadError::InvalidAlias)?
                .to_string();
            members.push(ArchiveMember::Alias {
                canonical_target: canonical_archive_target(&path, &target)?,
                path,
            });
        } else if entry_type.is_file() {
            members.push(ArchiveMember::File { path });
        }
        if members.len() > limits.max_files {
            return Err(UploadError::TooManyFiles);
        }
    }
    Ok(members)
}

#[allow(dead_code)]
fn canonical_archive_target(path: &str, target: &str) -> Result<String, UploadError> {
    if target.is_empty()
        || target.len() > MAX_ALIAS_TARGET_BYTES
        || target.starts_with(['/', '\\'])
        || target.contains('\\')
        || target.chars().any(char::is_control)
        || looks_like_external_archive_target(target)
    {
        return Err(UploadError::InvalidAlias);
    }
    let mut parts = path.rsplit_once('/').map_or_else(Vec::new, |(parent, _)| {
        parent.split('/').collect::<Vec<_>>()
    });
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(UploadError::InvalidAlias);
                }
            }
            component => parts.push(component),
        }
    }
    if parts.is_empty() {
        return Err(UploadError::InvalidAlias);
    }
    let canonical = parts.join("/");
    safe_rel_path(&canonical).map_err(|_| UploadError::InvalidAlias)?;
    if archive_reserved(&canonical) || is_noise_path(Path::new(&canonical)) {
        return Err(UploadError::InvalidAlias);
    }
    if relative_archive_target(path, &canonical).len() > MAX_ALIAS_TARGET_BYTES {
        return Err(UploadError::InvalidAlias);
    }
    Ok(canonical)
}

fn relative_archive_target(path: &str, target: &str) -> String {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let from = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').collect::<Vec<_>>()
    };
    let to = target.split('/').collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[allow(dead_code)]
fn looks_like_external_archive_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.contains("://")
        || [
            "data:",
            "file:",
            "ftp:",
            "ftps:",
            "git:",
            "http:",
            "https:",
            "javascript:",
            "mailto:",
            "ssh:",
            "ws:",
            "wss:",
        ]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

#[allow(dead_code)]
fn archive_reserved(path: &str) -> bool {
    const RESERVED: [&str; 7] = [
        "FILES",
        "HASH",
        "UNDO",
        "EXPIRES",
        "symbol.toml",
        ".symbol-token",
        ".symbol-claim",
    ];
    RESERVED.contains(&path.rsplit('/').next().unwrap_or(path))
}

#[allow(dead_code)]
fn validate_archive_members(mut members: Vec<ArchiveMember>) -> Result<ArchivePlan, UploadError> {
    if members.is_empty() {
        return Err(UploadError::EmptyArchive);
    }
    members.sort_unstable_by(|left, right| member_path(left).cmp(member_path(right)));
    for pair in members.windows(2) {
        if member_path(&pair[0]) == member_path(&pair[1]) {
            return Err(UploadError::InvalidAlias);
        }
    }
    for member in &members {
        if matches!(member, ArchiveMember::Alias { .. }) {
            let path = member_path(member);
            if archive_reserved(path) {
                return Err(UploadError::ReservedPath);
            }
            if path.chars().any(char::is_control) || is_noise_path(Path::new(path)) {
                return Err(UploadError::InvalidAlias);
            }
        }
    }
    let aliases = members
        .iter()
        .filter_map(|member| match member {
            ArchiveMember::Alias {
                path,
                canonical_target,
            } => Some((path.clone(), canonical_target.clone())),
            ArchiveMember::File { .. } => None,
        })
        .collect::<BTreeMap<_, _>>();
    for path in aliases.keys() {
        if members.iter().map(member_path).any(|member| {
            member
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/'))
        }) {
            return Err(UploadError::InvalidAlias);
        }
        let target = &aliases[path];
        if path
            .strip_prefix(target)
            .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(UploadError::AliasCycle);
        }
    }
    for path in aliases.keys() {
        let mut current = path.clone();
        let mut visited = HashSet::new();
        let mut complete = false;
        for _ in 0..64 {
            if !visited.insert(current.clone()) {
                return Err(UploadError::AliasCycle);
            }
            let Some((prefix, target)) = archive_alias_substitution(&aliases, &current) else {
                complete = true;
                break;
            };
            current = format!("{target}{}", &current[prefix.len()..]);
        }
        if !complete {
            return Err(UploadError::AliasCycle);
        }
        let resolves_directory = members.iter().map(member_path).any(|member| {
            member
                .strip_prefix(&current)
                .is_some_and(|suffix| suffix.starts_with('/'))
        });
        if resolves_directory
            && path
                .strip_prefix(&current)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(UploadError::AliasCycle);
        }
    }
    Ok(ArchivePlan { members })
}

#[allow(dead_code)]
fn member_path(member: &ArchiveMember) -> &str {
    match member {
        ArchiveMember::File { path } | ArchiveMember::Alias { path, .. } => path,
    }
}

#[allow(dead_code)]
fn archive_alias_substitution<'a>(
    aliases: &'a BTreeMap<String, String>,
    path: &str,
) -> Option<(&'a str, &'a str)> {
    if let Some((path, target)) = aliases.get_key_value(path) {
        return Some((path, target));
    }
    let mut end = path.len();
    while let Some(slash) = path[..end].rfind('/') {
        if let Some((path, target)) = aliases.get_key_value(&path[..slash]) {
            return Some((path, target));
        }
        end = slash;
    }
    None
}

#[cfg(test)]
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, UploadError> {
    let limits = archive_limits();
    let mut dec = GzDecoder::new(Cursor::new(bytes));
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = dec.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > limits.max_extracted {
            return Err(UploadError::TooLarge);
        }
    }
    Ok(out)
}

#[cfg(test)]
fn extract_zip(dest: &Path, bytes: &[u8]) -> Result<usize, UploadError> {
    extract_zip_reader(dest, Cursor::new(bytes))
}

fn extract_zip_reader<R: Read + Seek>(dest: &Path, reader: R) -> Result<usize, UploadError> {
    let limits = archive_limits();
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut files = 0usize;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if !file.is_file() {
            continue;
        }
        let Some(enclosed) = file.enclosed_name() else {
            return Err(UploadError::Path(PathError::Invalid));
        };
        if is_noise_path(&enclosed) {
            continue;
        }
        let rel = safe_rel_path(&enclosed.to_string_lossy())?;
        let size = file.size();
        total = total.saturating_add(size);
        if total > limits.max_extracted {
            return Err(UploadError::TooLarge);
        }
        if !write_kept_file(dest, &rel, &mut file)? {
            total = total.saturating_sub(size);
            continue;
        }
        files += 1;
        if files > limits.max_files {
            return Err(UploadError::TooManyFiles);
        }
    }
    if files == 0 {
        return Err(UploadError::EmptyArchive);
    }
    strip_single_root(dest)?;
    Ok(files)
}

fn extract_tar<R: Read>(dest: &Path, reader: R) -> Result<usize, UploadError> {
    let limits = archive_limits();
    let mut archive = tar::Archive::new(reader);
    let mut files = 0usize;
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?;
        if is_noise_path(&path) {
            continue;
        }
        let rel = safe_rel_path(&path.to_string_lossy())?;
        let size = entry.header().size()?;
        total = total.saturating_add(size);
        if total > limits.max_extracted {
            return Err(UploadError::TooLarge);
        }
        if !write_kept_file(dest, &rel, &mut entry)? {
            total = total.saturating_sub(size);
            continue;
        }
        files += 1;
        if files > limits.max_files {
            return Err(UploadError::TooManyFiles);
        }
    }
    if files == 0 {
        return Err(UploadError::EmptyArchive);
    }
    strip_single_root(dest)?;
    Ok(files)
}

fn write_kept_file<R: Read>(dest: &Path, rel: &Path, reader: &mut R) -> Result<bool, UploadError> {
    let mut buf = [0u8; 64 * 1024];
    let n = reader.read(&mut buf)?;
    if looks_like_apple_fork(&buf[..n]) {
        return Ok(false);
    }
    let path = dest.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::fs::File::create(path)?;
    if n > 0 {
        out.write_all(&buf[..n])?;
        std::io::copy(reader, &mut out)?;
    }
    Ok(true)
}

fn strip_single_root(dest: &Path) -> Result<(), UploadError> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dest)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            dirs.push(entry.file_name());
        } else {
            files.push(entry.file_name());
        }
    }
    if dirs.len() != 1 || !files.is_empty() {
        return Ok(());
    }
    let inner = dest.join(&dirs[0]);
    let staging = dest.join(".strip-root");
    std::fs::rename(&inner, &staging)?;
    for entry in std::fs::read_dir(&staging)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        std::fs::rename(entry.path(), to)?;
    }
    std::fs::remove_dir_all(staging)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_plans_preserve_safe_links_independent_of_member_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("links.tar");
        {
            let mut archive = tar::Builder::new(std::fs::File::create(&path).unwrap());
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("../assets/app.js").unwrap();
            link.set_cksum();
            archive
                .append_data(&mut link, "current/app.js", std::io::empty())
                .unwrap();
            let mut file = tar::Header::new_gnu();
            file.set_size(3);
            file.set_mode(0o644);
            file.set_cksum();
            archive
                .append_data(&mut file, "assets/app.js", b"app".as_slice())
                .unwrap();
            archive.finish().unwrap();
        }
        let plan = plan_archive(&path, Kind::Tar).unwrap();
        assert_eq!(
            plan.members,
            vec![
                ArchiveMember::File {
                    path: "assets/app.js".to_string()
                },
                ArchiveMember::Alias {
                    path: "current/app.js".to_string(),
                    canonical_target: "assets/app.js".to_string(),
                },
            ]
        );
    }

    #[test]
    fn archive_plans_reject_root_escape_and_complete_graph_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let escaped = dir.path().join("escaped.tar");
        {
            let mut archive = tar::Builder::new(std::fs::File::create(&escaped).unwrap());
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("../../outside").unwrap();
            link.set_cksum();
            archive
                .append_data(&mut link, "link", std::io::empty())
                .unwrap();
            archive.finish().unwrap();
        }
        assert!(matches!(
            plan_archive(&escaped, Kind::Tar),
            Err(UploadError::InvalidAlias)
        ));

        let cyclic = dir.path().join("cyclic.zip");
        {
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&cyclic).unwrap());
            let link = zip::write::SimpleFileOptions::default().unix_permissions(0o777);
            archive.add_symlink("a", "b", link).unwrap();
            archive.add_symlink("b", "a", link).unwrap();
            archive.finish().unwrap();
        }
        assert!(matches!(
            plan_archive(&cyclic, Kind::Zip),
            Err(UploadError::AliasCycle)
        ));
    }

    #[test]
    fn archive_plans_reject_containment_cycles_and_prefix_shadowing_in_any_order() {
        assert!(matches!(
            validate_archive_members(vec![ArchiveMember::Alias {
                path: "dir/link".to_string(),
                canonical_target: "dir".to_string(),
            }]),
            Err(UploadError::AliasCycle)
        ));
        assert!(matches!(
            validate_archive_members(vec![
                ArchiveMember::Alias {
                    path: "root-link".to_string(),
                    canonical_target: "dir".to_string(),
                },
                ArchiveMember::Alias {
                    path: "dir/back".to_string(),
                    canonical_target: "root-link".to_string(),
                },
            ]),
            Err(UploadError::AliasCycle)
        ));
        for members in [
            vec![
                ArchiveMember::Alias {
                    path: "shadow".to_string(),
                    canonical_target: "target".to_string(),
                },
                ArchiveMember::File {
                    path: "shadow/child".to_string(),
                },
            ],
            vec![
                ArchiveMember::File {
                    path: "shadow/child".to_string(),
                },
                ArchiveMember::Alias {
                    path: "shadow".to_string(),
                    canonical_target: "target".to_string(),
                },
            ],
        ] {
            assert!(matches!(
                validate_archive_members(members),
                Err(UploadError::InvalidAlias)
            ));
        }
    }

    #[test]
    fn zip_alias_targets_preserve_significant_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("whitespace.zip");
        {
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            archive.start_file(" target ", options).unwrap();
            archive.write_all(b"x").unwrap();
            archive
                .add_symlink("link", " target ", options.unix_permissions(0o777))
                .unwrap();
            archive.finish().unwrap();
        }
        let plan = plan_archive(&path, Kind::Zip).unwrap();
        assert!(plan.members.contains(&ArchiveMember::Alias {
            path: "link".to_string(),
            canonical_target: " target ".to_string(),
        }));
    }

    #[test]
    fn zip_alias_target_limit_rejects_one_byte_over_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized-link.zip");
        {
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            archive
                .add_symlink(
                    "link",
                    "x".repeat(MAX_ALIAS_TARGET_BYTES + 1),
                    zip::write::SimpleFileOptions::default().unix_permissions(0o777),
                )
                .unwrap();
            archive.finish().unwrap();
        }
        assert!(matches!(
            plan_archive(&path, Kind::Zip),
            Err(UploadError::InvalidAlias)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn tar_alias_targets_reject_non_utf8() {
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("non-utf8.tar");
        {
            let mut archive = tar::Builder::new(std::fs::File::create(&path).unwrap());
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            archive
                .append_link(
                    &mut header,
                    "link",
                    std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])),
                )
                .unwrap();
            archive.finish().unwrap();
        }
        assert!(matches!(
            plan_archive(&path, Kind::Tar),
            Err(UploadError::InvalidAlias)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn tar_member_paths_reject_non_utf8_without_lossy_conversion() {
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("non-utf8-path.tar");
        {
            let mut archive = tar::Builder::new(std::fs::File::create(&path).unwrap());
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            archive
                .append_data(
                    &mut header,
                    std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])),
                    b"x".as_slice(),
                )
                .unwrap();
            archive.finish().unwrap();
        }
        assert!(matches!(
            plan_archive(&path, Kind::Tar),
            Err(UploadError::Path(PathError::Invalid))
        ));
    }

    #[test]
    fn sniffs_html_and_zip() {
        assert_eq!(sniff(b"<!doctype html><h1>x</h1>", None, None), Kind::Html);
        assert_eq!(sniff(b"not html", None, Some("page.html")), Kind::Html);
        assert_eq!(sniff(b"PK\x03\x04rest", None, None), Kind::Zip);
        assert_eq!(sniff(&[0x1f, 0x8b, 0x08], None, None), Kind::Gzip);
    }

    #[test]
    fn zip_without_unpack_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        write_payload(
            dir.path(),
            b"PK\x03\x04rest",
            Kind::Zip,
            Some("site.zip"),
            false,
        )
        .unwrap();
        assert!(dir.path().join("site.zip").is_file());
        assert!(!dir.path().join("index.html").exists());
    }

    #[test]
    fn zip_is_unpacked() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("index.html", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"<h1>z</h1>").unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();
        write_payload(dir.path(), &bytes, Kind::Zip, Some("site.zip"), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("index.html")).unwrap(),
            "<h1>z</h1>"
        );
        assert!(!dir.path().join("site.zip").exists());
    }

    #[test]
    fn spooled_zip_is_unpacked_without_loading_archive_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("upload.zip");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("site/index.html", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"streamed").unwrap();
            zip.finish().unwrap();
        }
        let output = dir.path().join("output");
        std::fs::create_dir(&output).unwrap();
        assert_eq!(
            write_payload_file(&output, &archive_path, Kind::Zip, Some("upload.zip"), true)
                .unwrap(),
            1
        );
        assert_eq!(
            std::fs::read(output.join("index.html")).unwrap(),
            b"streamed"
        );
    }

    #[test]
    fn zip_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("index.html", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"<h1>ok</h1>").unwrap();
            zip.start_file("._index.html", opts).unwrap();
            std::io::Write::write_all(&mut zip, &[0x00, 0x05, 0x16, 0x07, 0, 2, 0, 0]).unwrap();
            zip.start_file("._.", opts).unwrap();
            std::io::Write::write_all(&mut zip, &[0x00, 0x05, 0x16, 0x07, 0, 2, 0, 0]).unwrap();
            zip.start_file(".DS_Store", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"ds").unwrap();
            zip.start_file("__MACOSX/._index.html", opts).unwrap();
            std::io::Write::write_all(&mut zip, &[0x00, 0x05, 0x16, 0x07]).unwrap();
            zip.start_file("desktop.ini", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"[.ShellClassInfo]").unwrap();
            zip.start_file("keep.bin", opts).unwrap();
            std::io::Write::write_all(&mut zip, &[0x00, 0x05, 0x16, 0x07, 0, 2, 0, 0]).unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();
        write_payload(dir.path(), &bytes, Kind::Zip, Some("site.zip"), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("index.html")).unwrap(),
            "<h1>ok</h1>"
        );
        assert!(!dir.path().join("._index.html").exists());
        assert!(!dir.path().join("._.").exists());
        assert!(!dir.path().join(".DS_Store").exists());
        assert!(!dir.path().join("desktop.ini").exists());
        assert!(!dir.path().join("keep.bin").exists());
        assert!(!dir.path().join("__MACOSX").exists());
    }

    #[test]
    fn zip_slip_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("../evil.txt", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"nope").unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();
        let err = extract_zip(dir.path(), &bytes).unwrap_err();
        assert!(matches!(err, UploadError::Path(_) | UploadError::Zip(_)));
        assert!(!dir.path().join("evil.txt").exists());
    }

    #[test]
    fn html_writes_index() {
        let dir = tempfile::tempdir().unwrap();
        write_payload(dir.path(), b"<h1>hi</h1>", Kind::Html, None, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("index.html")).unwrap(),
            "<h1>hi</h1>"
        );
    }

    #[test]
    fn file_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        write_payload(dir.path(), b"hello", Kind::File, Some("notes.txt"), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn unpack_on_html_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_payload(dir.path(), b"<h1>hi</h1>", Kind::Html, None, true).unwrap_err();
        assert!(matches!(err, UploadError::NotArchive));
    }

    #[test]
    fn single_junk_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            write_payload(dir.path(), b"ds", Kind::File, Some(".DS_Store"), false).unwrap_err();
        assert!(matches!(err, UploadError::Junk));
        let apple = [0x00, 0x05, 0x16, 0x07, 0, 2, 0, 0];
        let err =
            write_payload(dir.path(), &apple, Kind::File, Some("meta.bin"), false).unwrap_err();
        assert!(matches!(err, UploadError::Junk));
    }

    #[test]
    fn opaque_supported_archives_reject_embedded_management_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("site.zip");
        let management =
            "sym_mgmt_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("index.html", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(management.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            reject_secrets_in_opaque_archive(&archive_path, Kind::Zip),
            Err(UploadError::OpaqueSecret)
        ));

        let clean_path = directory.path().join("clean.zip");
        {
            let file = std::fs::File::create(&clean_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("index.html", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"clean").unwrap();
            zip.finish().unwrap();
        }
        reject_secrets_in_opaque_archive(&clean_path, Kind::Zip).unwrap();
    }
}
