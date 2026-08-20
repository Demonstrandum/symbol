#[cfg(test)]
use std::io::Cursor;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::Path;

use flate2::read::GzDecoder;

#[cfg(test)]
use crate::pathutil::is_junk;
use crate::pathutil::{PathError, is_noise_path, looks_like_apple_fork, safe_rel_path};
use crate::sanitize;

const MAX_FILES: usize = 5000;
const MAX_EXTRACTED: u64 = 80 * 1024 * 1024;

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
    #[error("{0}")]
    Path(#[from] PathError),
    #[error("error: zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("error: {0}")]
    Io(#[from] std::io::Error),
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
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED {
            return Err(UploadError::TooLarge);
        }
        if sanitize::count_reader(&mut entry)?.total() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn tar_contains_secret(reader: impl Read) -> Result<bool, UploadError> {
    let mut archive = tar::Archive::new(reader);
    let mut total = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        total = total.saturating_add(entry.header().size()?);
        if total > MAX_EXTRACTED {
            return Err(UploadError::TooLarge);
        }
        if sanitize::count_reader(&mut entry)?.total() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn gzip_contains_secret(source: &Path) -> Result<bool, UploadError> {
    let decoder = GzDecoder::new(std::fs::File::open(source)?);
    let mut reader = BufReader::new(decoder);
    if looks_like_tar(reader.fill_buf()?) {
        return tar_contains_secret(reader);
    }
    let mut limited = reader.take(MAX_EXTRACTED + 1);
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
    let copied = std::io::copy(&mut reader.take(MAX_EXTRACTED + 1), &mut output)?;
    if copied > MAX_EXTRACTED {
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

#[cfg(test)]
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, UploadError> {
    let mut dec = GzDecoder::new(Cursor::new(bytes));
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = dec.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > MAX_EXTRACTED {
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
        if total > MAX_EXTRACTED {
            return Err(UploadError::TooLarge);
        }
        if !write_kept_file(dest, &rel, &mut file)? {
            total = total.saturating_sub(size);
            continue;
        }
        files += 1;
        if files > MAX_FILES {
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
        if total > MAX_EXTRACTED {
            return Err(UploadError::TooLarge);
        }
        if !write_kept_file(dest, &rel, &mut entry)? {
            total = total.saturating_sub(size);
            continue;
        }
        files += 1;
        if files > MAX_FILES {
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
