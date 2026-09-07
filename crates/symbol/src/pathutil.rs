use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("error: empty path")]
    Empty,
    #[error("error: path is not allowed")]
    Invalid,
}

pub fn safe_rel_path(raw: &str) -> Result<PathBuf, PathError> {
    let raw = raw.replace('\\', "/");
    if raw.is_empty() || raw.contains('\0') {
        return Err(PathError::Empty);
    }
    let mut out = PathBuf::new();
    for part in raw.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') {
            return Err(PathError::Invalid);
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }
    Ok(out)
}

pub fn is_noise_path(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(s) => is_noise_name(&s.to_string_lossy()),
        _ => false,
    })
}

pub fn is_junk(path: &Path, bytes: Option<&[u8]>) -> bool {
    is_noise_path(path) || bytes.is_some_and(looks_like_apple_fork)
}

/// `AppleSingle` (`00 05 16 00`) / `AppleDouble` (`00 05 16 07`) resource-fork files.
pub const fn looks_like_apple_fork(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes[0] == 0x00
        && bytes[1] == 0x05
        && bytes[2] == 0x16
        && (bytes[3] == 0x00 || bytes[3] == 0x07)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlSuffix {
    Html,
    Htm,
}

pub fn html_suffix(name: &str) -> Option<(&str, HtmlSuffix)> {
    if let Some(stem) = name.strip_suffix(".html") {
        return (!stem.is_empty() && !stem.ends_with('/')).then_some((stem, HtmlSuffix::Html));
    }
    if let Some(stem) = name.strip_suffix(".htm") {
        return (!stem.is_empty() && !stem.ends_with('/')).then_some((stem, HtmlSuffix::Htm));
    }
    None
}

pub fn pretty_html_name<'a>(name: &'a str, occupied: impl Fn(&str) -> bool) -> &'a str {
    match html_suffix(name) {
        Some((stem, HtmlSuffix::Html)) if !occupied(stem) => stem,
        Some((stem, HtmlSuffix::Htm)) if !occupied(stem) && !occupied(&format!("{stem}.html")) => {
            stem
        }
        _ => name,
    }
}

fn is_noise_name(name: &str) -> bool {
    if name.starts_with("._") || name == "Icon\r" {
        return true;
    }
    name.eq_ignore_ascii_case("__MACOSX")
        || name.eq_ignore_ascii_case(".AppleDouble")
        || name.eq_ignore_ascii_case(".DS_Store")
        || name.eq_ignore_ascii_case(".LSOverride")
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("ehthumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zip_slip() {
        assert!(safe_rel_path("../etc/passwd").is_err());
        assert!(safe_rel_path("foo/../../etc/passwd").is_err());
        assert!(safe_rel_path("/etc/passwd").is_ok()); // leading slash stripped
        assert_eq!(
            safe_rel_path("/etc/passwd").unwrap(),
            PathBuf::from("etc/passwd")
        );
    }

    #[test]
    fn keeps_nested() {
        assert!(safe_rel_path("./css/../css/style.css").is_err());
        assert_eq!(
            safe_rel_path("css/style.css").unwrap(),
            PathBuf::from("css/style.css")
        );
        assert_eq!(
            safe_rel_path("./css/style.css").unwrap(),
            PathBuf::from("css/style.css")
        );
    }

    #[test]
    fn detects_os_metadata() {
        assert!(is_noise_path(Path::new("._.")));
        assert!(is_noise_path(Path::new("._index.html")));
        assert!(is_noise_path(Path::new("css/._style.css")));
        assert!(is_noise_path(Path::new("__MACOSX/foo")));
        assert!(is_noise_path(Path::new(".DS_Store")));
        assert!(is_noise_path(Path::new("desktop.ini")));
        assert!(is_noise_path(Path::new("Thumbs.db")));
        assert!(!is_noise_path(Path::new("index.html")));
        assert!(!is_noise_path(Path::new(".htaccess")));
        assert!(!is_noise_path(Path::new("css/style.css")));
    }

    #[test]
    fn detects_apple_fork_bytes() {
        let appledouble = [
            0x00, 0x05, 0x16, 0x07, 0x00, 0x02, 0x00, 0x00, b'M', b'a', b'c', b' ', b'O', b'S',
            b' ', b'X',
        ];
        assert!(looks_like_apple_fork(&appledouble));
        assert!(is_junk(Path::new("metadata.bin"), Some(&appledouble)));
        assert!(!is_junk(Path::new("index.html"), Some(b"<h1>ok</h1>")));
        assert!(is_junk(Path::new("._index.html"), Some(b"<h1>ok</h1>")));
    }

    #[test]
    fn html_suffix_is_exact_and_pretty_name_is_unambiguous() {
        assert_eq!(html_suffix("about.html"), Some(("about", HtmlSuffix::Html)));
        assert_eq!(html_suffix("about.htm"), Some(("about", HtmlSuffix::Htm)));
        assert_eq!(html_suffix("About.HTML"), None);
        assert_eq!(html_suffix("about.xhtml"), None);
        assert_eq!(html_suffix(".html"), None);
        assert_eq!(
            pretty_html_name("about.html", |name| name == "about.html"),
            "about"
        );
        assert_eq!(
            pretty_html_name("about.html", |name| name == "about" || name == "about.html"),
            "about.html"
        );
        assert_eq!(
            pretty_html_name("about.htm", |name| name == "about.htm"
                || name == "about.html"),
            "about.htm"
        );
        assert_eq!(
            pretty_html_name("about.htm", |name| name == "about.htm"),
            "about"
        );
    }
}
