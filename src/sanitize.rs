use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::secrets::{CLAIM_TOKEN_PREFIX, MANAGEMENT_TOKEN_PREFIX, TOKEN_HEX_LEN, TokenKind};

pub const SCAN_ALL_FILES_THROUGH: usize = 1024 * 1024;
#[cfg(test)]
pub const MANAGEMENT_SIDECAR: &str = ".symbol-token";
#[cfg(test)]
pub const CLAIM_SIDECAR: &str = ".symbol-claim";
const TOKEN_WINDOW: usize = MANAGEMENT_TOKEN_PREFIX.len() + TOKEN_HEX_LEN + 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct TokenCounts {
    pub management: usize,
    pub claim: usize,
}

impl TokenCounts {
    #[must_use]
    pub const fn total(self) -> usize {
        self.management + self.claim
    }

    const fn increment(&mut self, kind: TokenKind) {
        match kind {
            TokenKind::Management => self.management += 1,
            TokenKind::Claim => self.claim += 1,
        }
    }
}

pub struct SanitizedBytes {
    bytes: Vec<u8>,
    counts: TokenCounts,
}

impl SanitizedBytes {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn counts(&self) -> TokenCounts {
        self.counts
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanClass {
    Small,
    Text,
    Binary,
}

#[cfg(test)]
impl ScanClass {
    #[must_use]
    pub const fn should_scan(self) -> bool {
        !matches!(self, Self::Binary)
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedSidecar {
    ManagementToken,
    ClaimToken,
}

#[must_use]
pub fn redact_tokens(input: &[u8]) -> SanitizedBytes {
    let mut output = Vec::new();
    let mut counts = TokenCounts::default();
    let mut copied_through = 0;
    let mut cursor = 0;

    while cursor < input.len() {
        let Some((kind, prefix_len)) = token_prefix_at(&input[cursor..]) else {
            cursor += 1;
            continue;
        };
        let payload_start = cursor + prefix_len;
        let payload_end = payload_start + TOKEN_HEX_LEN;
        if payload_end > input.len()
            || !input[payload_start..payload_end]
                .iter()
                .all(u8::is_ascii_hexdigit)
            || input[payload_start..payload_end]
                .iter()
                .any(u8::is_ascii_uppercase)
            || input.get(payload_end).is_some_and(u8::is_ascii_hexdigit)
        {
            cursor += 1;
            continue;
        }

        output.extend_from_slice(&input[copied_through..payload_start]);
        output.extend(std::iter::repeat_n(b'*', TOKEN_HEX_LEN));
        counts.increment(kind);
        copied_through = payload_end;
        cursor = payload_end;
    }

    if counts.total() == 0 {
        output.extend_from_slice(input);
    } else {
        output.extend_from_slice(&input[copied_through..]);
    }
    SanitizedBytes {
        bytes: output,
        counts,
    }
}

#[cfg(test)]
#[must_use]
pub fn count_tokens(input: &[u8]) -> TokenCounts {
    scan_tokens(input, |_, _| {})
}

#[cfg(test)]
pub fn scan_tokens(input: &[u8], mut found: impl FnMut(TokenKind, usize)) -> TokenCounts {
    let mut counts = TokenCounts::default();
    let mut cursor = 0;

    while cursor < input.len() {
        let Some((kind, prefix_len)) = token_prefix_at(&input[cursor..]) else {
            cursor += 1;
            continue;
        };
        let payload_start = cursor + prefix_len;
        let payload_end = payload_start + TOKEN_HEX_LEN;
        if payload_end <= input.len()
            && input[payload_start..payload_end]
                .iter()
                .all(u8::is_ascii_hexdigit)
            && !input[payload_start..payload_end]
                .iter()
                .any(u8::is_ascii_uppercase)
            && !input.get(payload_end).is_some_and(u8::is_ascii_hexdigit)
        {
            counts.increment(kind);
            found(kind, cursor);
            cursor = payload_end;
        } else {
            cursor += 1;
        }
    }
    counts
}

#[cfg(test)]
#[must_use]
pub fn classify_for_scan(bytes: &[u8]) -> ScanClass {
    if bytes.len() <= SCAN_ALL_FILES_THROUGH {
        ScanClass::Small
    } else if is_text_like(bytes) {
        ScanClass::Text
    } else {
        ScanClass::Binary
    }
}

#[cfg(test)]
#[must_use]
pub fn is_text_like(bytes: &[u8]) -> bool {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return false;
    }

    let suspicious_controls = bytes
        .iter()
        .filter(|byte| byte.is_ascii_control() && !matches!(**byte, b'\n' | b'\r' | b'\t' | 0x0c))
        .count();
    suspicious_controls <= bytes.len() / 100
}

#[cfg(test)]
#[must_use]
pub fn reserved_sidecar(path: &Path) -> Option<ReservedSidecar> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(MANAGEMENT_SIDECAR) => Some(ReservedSidecar::ManagementToken),
        Some(CLAIM_SIDECAR) => Some(ReservedSidecar::ClaimToken),
        _ => None,
    }
}

#[cfg(test)]
#[must_use]
pub fn is_reserved_sidecar(path: &Path) -> bool {
    reserved_sidecar(path).is_some()
}

pub fn sanitize_file(path: &Path) -> io::Result<TokenCounts> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > SCAN_ALL_FILES_THROUGH as u64 && !is_text_file(path)? {
        return Ok(TokenCounts::default());
    }

    let temporary = sanitized_path(path);
    let counts = {
        let input = fs::File::open(path)?;
        let mut output = fs::File::create(&temporary)?;
        let counts = redact_reader(input, &mut output)?;
        output.sync_all()?;
        counts
    };
    if counts.total() == 0 {
        fs::remove_file(temporary)?;
    } else {
        fs::rename(temporary, path)?;
    }
    Ok(counts)
}

pub fn count_reader(reader: impl Read) -> io::Result<TokenCounts> {
    scan_reader(reader, None::<io::Sink>)
}

fn redact_reader(reader: impl Read, output: impl Write) -> io::Result<TokenCounts> {
    scan_reader(reader, Some(output))
}

fn scan_reader(mut reader: impl Read, mut output: Option<impl Write>) -> io::Result<TokenCounts> {
    let mut pending = Vec::with_capacity(64 * 1024 + TOKEN_WINDOW);
    let mut chunk = [0_u8; 64 * 1024];
    let mut counts = TokenCounts::default();
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        if pending.len() <= TOKEN_WINDOW {
            continue;
        }
        let sanitized = redact_tokens(&pending);
        counts.management += sanitized.counts.management;
        counts.claim += sanitized.counts.claim;
        let split = sanitized.as_bytes().len() - TOKEN_WINDOW;
        if let Some(writer) = output.as_mut() {
            writer.write_all(&sanitized.as_bytes()[..split])?;
        }
        pending = sanitized.as_bytes()[split..].to_vec();
    }
    let sanitized = redact_tokens(&pending);
    counts.management += sanitized.counts.management;
    counts.claim += sanitized.counts.claim;
    if let Some(writer) = output.as_mut() {
        writer.write_all(sanitized.as_bytes())?;
    }
    Ok(counts)
}

fn is_text_file(path: &Path) -> io::Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut chunk = [0_u8; 64 * 1024];
    let mut utf8_tail = Vec::with_capacity(4);
    let mut total = 0_usize;
    let mut controls = 0_usize;
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        controls = controls.saturating_add(
            chunk[..read]
                .iter()
                .filter(|byte| {
                    byte.is_ascii_control() && !matches!(**byte, b'\n' | b'\r' | b'\t' | 0x0c)
                })
                .count(),
        );
        if chunk[..read].contains(&0) {
            return Ok(false);
        }
        utf8_tail.extend_from_slice(&chunk[..read]);
        match std::str::from_utf8(&utf8_tail) {
            Ok(_) => utf8_tail.clear(),
            Err(error) if error.error_len().is_none() => {
                let incomplete = utf8_tail.split_off(error.valid_up_to());
                utf8_tail = incomplete;
            }
            Err(_) => return Ok(false),
        }
    }
    Ok(utf8_tail.is_empty() && controls <= total / 100)
}

fn sanitized_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".symbol-sanitized");
    PathBuf::from(temporary)
}

fn token_prefix_at(bytes: &[u8]) -> Option<(TokenKind, usize)> {
    if bytes.starts_with(MANAGEMENT_TOKEN_PREFIX.as_bytes()) {
        Some((TokenKind::Management, MANAGEMENT_TOKEN_PREFIX.len()))
    } else if bytes.starts_with(CLAIM_TOKEN_PREFIX.as_bytes()) {
        Some((TokenKind::Claim, CLAIM_TOKEN_PREFIX.len()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANAGEMENT: &str =
        "sym_mgmt_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const CLAIM: &str =
        "sym_claim_fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    #[test]
    fn redacts_only_payloads_and_reports_kind_counts() {
        let input = format!("management={MANAGEMENT}\nclaim={CLAIM}\nagain={MANAGEMENT}");
        let sanitized = redact_tokens(input.as_bytes());
        let output = std::str::from_utf8(sanitized.as_bytes()).unwrap();

        assert_eq!(
            output,
            format!(
                "management={MANAGEMENT_TOKEN_PREFIX}{stars}\n\
                 claim={CLAIM_TOKEN_PREFIX}{stars}\n\
                 again={MANAGEMENT_TOKEN_PREFIX}{stars}",
                stars = "*".repeat(TOKEN_HEX_LEN)
            )
        );
        assert_eq!(
            sanitized.counts(),
            TokenCounts {
                management: 2,
                claim: 1
            }
        );
        assert!(!output.contains("0123456789abcdef"));
        assert!(!output.contains("fedcba9876543210"));
    }

    #[test]
    fn handles_tokens_at_boundaries_and_back_to_back() {
        let input = format!("{MANAGEMENT}{CLAIM}");
        let sanitized = redact_tokens(input.as_bytes());

        assert_eq!(
            sanitized.as_bytes(),
            format!(
                "{MANAGEMENT_TOKEN_PREFIX}{stars}\
                 {CLAIM_TOKEN_PREFIX}{stars}",
                stars = "*".repeat(TOKEN_HEX_LEN)
            )
            .as_bytes()
        );
        assert_eq!(sanitized.counts().total(), 2);
    }

    #[test]
    fn preserves_input_exactly_when_no_token_is_present() {
        let input = b"\0binary\xff and ordinary text";
        let sanitized = redact_tokens(input);

        assert_eq!(sanitized.as_bytes(), input);
        assert_eq!(sanitized.counts(), TokenCounts::default());
    }

    #[test]
    fn ignores_malformed_or_embedded_longer_payloads() {
        let short = format!("{MANAGEMENT_TOKEN_PREFIX}abc");
        let uppercase = CLAIM.replace('f', "F");
        let longer = format!("{MANAGEMENT}0");
        let input = format!("{short} {uppercase} {longer}");
        let sanitized = redact_tokens(input.as_bytes());

        assert_eq!(sanitized.as_bytes(), input.as_bytes());
        assert_eq!(sanitized.counts().total(), 0);
    }

    #[test]
    fn scans_linearly_and_reports_offsets() {
        let input = format!("before {CLAIM} middle {MANAGEMENT} after");
        let mut occurrences = Vec::new();
        let counts = scan_tokens(input.as_bytes(), |kind, offset| {
            occurrences.push((kind, offset));
        });

        assert_eq!(counts.management, 1);
        assert_eq!(counts.claim, 1);
        assert_eq!(
            occurrences,
            vec![
                (TokenKind::Claim, "before ".len()),
                (
                    TokenKind::Management,
                    "before ".len() + CLAIM.len() + " middle ".len()
                )
            ]
        );
        assert_eq!(count_tokens(input.as_bytes()), counts);
    }

    #[test]
    fn all_files_through_one_mib_are_scanned() {
        let binary = vec![0_u8; SCAN_ALL_FILES_THROUGH];
        assert_eq!(classify_for_scan(&binary), ScanClass::Small);
        assert!(classify_for_scan(&binary).should_scan());
    }

    #[test]
    fn large_utf8_text_is_scanned() {
        let text = "A text line with λ.\n".repeat(SCAN_ALL_FILES_THROUGH / 10);
        assert!(text.len() > SCAN_ALL_FILES_THROUGH);
        assert!(is_text_like(text.as_bytes()));
        assert_eq!(classify_for_scan(text.as_bytes()), ScanClass::Text);
    }

    #[test]
    fn large_binary_content_is_not_scanned() {
        let mut binary = vec![b'a'; SCAN_ALL_FILES_THROUGH + 1];
        binary[SCAN_ALL_FILES_THROUGH / 2] = 0;
        assert!(!is_text_like(&binary));
        assert_eq!(classify_for_scan(&binary), ScanClass::Binary);

        binary[SCAN_ALL_FILES_THROUGH / 2] = 0xff;
        assert!(!is_text_like(&binary));
        assert_eq!(classify_for_scan(&binary), ScanClass::Binary);
    }

    #[test]
    fn text_heuristic_rejects_control_heavy_data() {
        let controls = vec![1_u8; 10_000];
        assert!(!is_text_like(&controls));
        assert!(is_text_like(b"hello\tworld\r\n"));
        assert!(is_text_like("valid utf-8: λ".as_bytes()));
    }

    #[test]
    fn recognizes_reserved_sidecars_by_final_component() {
        assert_eq!(
            reserved_sidecar(Path::new("site/.symbol-token")),
            Some(ReservedSidecar::ManagementToken)
        );
        assert_eq!(
            reserved_sidecar(Path::new(".symbol-claim")),
            Some(ReservedSidecar::ClaimToken)
        );
        assert!(is_reserved_sidecar(Path::new("nested/.symbol-token")));
        assert!(!is_reserved_sidecar(Path::new(".symbol-token.bak")));
        assert!(!is_reserved_sidecar(Path::new("symbol-token")));
    }

    #[test]
    fn sanitized_bytes_do_not_implement_plaintext_formatting() {
        fn accepts_bytes(_: &[u8]) {}
        accepts_bytes(redact_tokens(MANAGEMENT.as_bytes()).as_bytes());
    }

    #[test]
    fn file_sanitizer_handles_chunk_boundaries_and_large_text() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.txt");
        let mut bytes = vec![b'a'; 64 * 1024 - 12];
        bytes.extend_from_slice(MANAGEMENT.as_bytes());
        bytes.push(b' ');
        bytes.resize(SCAN_ALL_FILES_THROUGH + 1, b'b');
        std::fs::write(&path, &bytes).unwrap();

        let counts = sanitize_file(&path).unwrap();
        let stored = std::fs::read(path).unwrap();
        assert_eq!(counts.management, 1);
        assert_eq!(stored.len(), bytes.len());
        assert!(
            !stored
                .windows(MANAGEMENT.len())
                .any(|window| window == MANAGEMENT.as_bytes())
        );
        assert!(
            stored
                .windows(MANAGEMENT_TOKEN_PREFIX.len())
                .any(|window| { window == MANAGEMENT_TOKEN_PREFIX.as_bytes() })
        );
    }

    #[test]
    fn large_binary_files_are_left_opaque() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.bin");
        let mut bytes = vec![0_u8; SCAN_ALL_FILES_THROUGH + 1];
        bytes[..MANAGEMENT.len()].copy_from_slice(MANAGEMENT.as_bytes());
        std::fs::write(&path, &bytes).unwrap();

        assert_eq!(sanitize_file(&path).unwrap(), TokenCounts::default());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
