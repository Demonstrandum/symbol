const RESERVED: &[&str] = &["files", "stats", "symbol"];

const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const ID_LEN: usize = 4;

pub fn parse_site_name(raw: &str) -> Result<&str, NameError> {
    if raw.is_empty() || raw.len() > 63 {
        return Err(NameError::Invalid);
    }
    let bytes = raw.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        return Err(NameError::Invalid);
    }
    if !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return Err(NameError::Invalid);
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return Err(NameError::Invalid);
    }
    if RESERVED.contains(&raw) {
        return Err(NameError::Reserved);
    }
    Ok(raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("error: name must be 1-63 chars, lowercase alphanumeric, hyphens in the middle")]
    Invalid,
    #[error("error: that name is reserved")]
    Reserved,
}

pub fn generate_id(taken: impl Fn(&str) -> bool) -> String {
    for len in ID_LEN..ID_LEN + 4 {
        for _ in 0..32 {
            let id = random_id(len);
            if parse_site_name(&id).is_ok() && !taken(&id) {
                return id;
            }
        }
    }
    random_id(8)
}

fn random_id(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).expect("rng");
    bytes
        .iter()
        .map(|b| ID_ALPHABET[(*b as usize) % ID_ALPHABET.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn accepts_simple_names() {
        assert_eq!(parse_site_name("hello"), Ok("hello"));
        assert_eq!(parse_site_name("a"), Ok("a"));
        assert_eq!(parse_site_name("ab-cd-9"), Ok("ab-cd-9"));
    }

    #[test]
    fn rejects_bad_names() {
        assert!(parse_site_name("").is_err());
        assert!(parse_site_name("-hello").is_err());
        assert!(parse_site_name("hello-").is_err());
        assert!(parse_site_name("Hello").is_err());
        assert!(parse_site_name("files").is_err());
        assert!(parse_site_name("stats").is_err());
        assert!(parse_site_name("install.sh").is_err());
    }

    #[test]
    fn generated_id_is_short_and_valid() {
        let taken = HashSet::<String>::new();
        let id = generate_id(|n| taken.contains(n));
        assert_eq!(id.len(), ID_LEN);
        assert_eq!(parse_site_name(&id), Ok(id.as_str()));
    }

    #[test]
    fn skips_taken_ids() {
        let mut taken: HashSet<String> = HashSet::new();
        taken.insert(generate_id(|_| false));
        let id = generate_id(|n| taken.contains(n));
        assert!(!taken.contains(&id));
        assert_eq!(parse_site_name(&id), Ok(id.as_str()));
    }
}
