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
        Component::Normal(s) => {
            let s = s.to_string_lossy();
            s == "__MACOSX" || s == ".DS_Store" || s == "Thumbs.db"
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zip_slip() {
        assert!(safe_rel_path("../etc/passwd").is_err());
        assert!(safe_rel_path("foo/../../etc/passwd").is_err());
        assert!(safe_rel_path("/etc/passwd").is_ok()); // leading slash stripped
        assert_eq!(safe_rel_path("/etc/passwd").unwrap(), PathBuf::from("etc/passwd"));
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
}
