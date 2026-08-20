use std::fmt;
use std::str::FromStr;

use subtle::ConstantTimeEq;

pub const TOKEN_BYTES: usize = 32;
pub const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;
pub const MANAGEMENT_TOKEN_PREFIX: &str = "sym_mgmt_";
pub const CLAIM_TOKEN_PREFIX: &str = "sym_claim_";

const MANAGEMENT_HASH_CONTEXT: &str = "symbol management token hash v1";
const CLAIM_HASH_CONTEXT: &str = "symbol claim token hash v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Management,
    Claim,
}

#[derive(Clone)]
pub struct ManagementToken([u8; TOKEN_BYTES]);

#[derive(Clone)]
pub struct ClaimToken([u8; TOKEN_BYTES]);

#[cfg(test)]
pub enum Token {
    Management(ManagementToken),
    Claim(ClaimToken),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ManagementTokenHash([u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ClaimTokenHash([u8; 32]);

#[cfg(test)]
pub enum TokenHash {
    Management(ManagementTokenHash),
    Claim(ClaimTokenHash),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenParseError {
    #[error("token prefix is not recognized")]
    UnknownPrefix,
    #[error("token payload must contain exactly {TOKEN_HEX_LEN} hexadecimal bytes")]
    InvalidLength,
    #[error("token payload is not lowercase hexadecimal")]
    InvalidEncoding,
}

impl ManagementToken {
    /// Generates a token using the operating system's cryptographic random source.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot provide random bytes.
    pub fn generate() -> Result<Self, getrandom::Error> {
        random_bytes().map(Self)
    }

    /// Parses a canonical management token.
    ///
    /// # Errors
    ///
    /// Returns an error for a wrong prefix, wrong payload length, or non-lowercase-hex payload.
    pub fn parse(encoded: &str) -> Result<Self, TokenParseError> {
        parse_payload(encoded, MANAGEMENT_TOKEN_PREFIX).map(Self)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        encode_token(MANAGEMENT_TOKEN_PREFIX, &self.0)
    }

    #[must_use]
    pub fn hash(&self) -> ManagementTokenHash {
        ManagementTokenHash(blake3::derive_key(MANAGEMENT_HASH_CONTEXT, &self.0))
    }
}

impl ClaimToken {
    /// Generates a token using the operating system's cryptographic random source.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot provide random bytes.
    pub fn generate() -> Result<Self, getrandom::Error> {
        random_bytes().map(Self)
    }

    /// Parses a canonical claim token.
    ///
    /// # Errors
    ///
    /// Returns an error for a wrong prefix, wrong payload length, or non-lowercase-hex payload.
    pub fn parse(encoded: &str) -> Result<Self, TokenParseError> {
        parse_payload(encoded, CLAIM_TOKEN_PREFIX).map(Self)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        encode_token(CLAIM_TOKEN_PREFIX, &self.0)
    }

    #[must_use]
    pub fn hash(&self) -> ClaimTokenHash {
        ClaimTokenHash(blake3::derive_key(CLAIM_HASH_CONTEXT, &self.0))
    }
}

#[cfg(test)]
impl Token {
    /// Parses either supported token kind and retains its type.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown prefix or malformed token payload.
    pub fn parse(encoded: &str) -> Result<Self, TokenParseError> {
        if encoded.starts_with(MANAGEMENT_TOKEN_PREFIX) {
            ManagementToken::parse(encoded).map(Self::Management)
        } else if encoded.starts_with(CLAIM_TOKEN_PREFIX) {
            ClaimToken::parse(encoded).map(Self::Claim)
        } else {
            Err(TokenParseError::UnknownPrefix)
        }
    }

    #[must_use]
    pub const fn kind(&self) -> TokenKind {
        match self {
            Self::Management(_) => TokenKind::Management,
            Self::Claim(_) => TokenKind::Claim,
        }
    }

    #[must_use]
    pub fn hash(&self) -> TokenHash {
        match self {
            Self::Management(token) => TokenHash::Management(token.hash()),
            Self::Claim(token) => TokenHash::Claim(token.hash()),
        }
    }
}

impl ManagementTokenHash {
    #[must_use]
    pub fn verify(&self, candidate: &ManagementToken) -> bool {
        self.0.ct_eq(&candidate.hash().0).into()
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl ClaimTokenHash {
    #[must_use]
    pub fn verify(&self, candidate: &ClaimToken) -> bool {
        self.0.ct_eq(&candidate.hash().0).into()
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl FromStr for ManagementToken {
    type Err = TokenParseError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded)
    }
}

impl FromStr for ClaimToken {
    type Err = TokenParseError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded)
    }
}

#[cfg(test)]
impl FromStr for Token {
    type Err = TokenParseError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded)
    }
}

impl fmt::Debug for ManagementToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ManagementToken")
            .field(&Redacted)
            .finish()
    }
}

impl fmt::Debug for ClaimToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ClaimToken")
            .field(&Redacted)
            .finish()
    }
}

#[cfg(test)]
impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Management(token) => token.fmt(formatter),
            Self::Claim(token) => token.fmt(formatter),
        }
    }
}

impl fmt::Debug for ManagementTokenHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ManagementTokenHash([REDACTED])")
    }
}

impl fmt::Debug for ClaimTokenHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClaimTokenHash([REDACTED])")
    }
}

#[cfg(test)]
impl fmt::Debug for TokenHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Management(hash) => hash.fmt(formatter),
            Self::Claim(hash) => hash.fmt(formatter),
        }
    }
}

struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED: 32 bytes]")
    }
}

fn random_bytes() -> Result<[u8; TOKEN_BYTES], getrandom::Error> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}

fn parse_payload(
    encoded: &str,
    expected_prefix: &str,
) -> Result<[u8; TOKEN_BYTES], TokenParseError> {
    let payload = encoded
        .strip_prefix(expected_prefix)
        .ok_or(TokenParseError::UnknownPrefix)?;
    if payload.len() != TOKEN_HEX_LEN {
        return Err(TokenParseError::InvalidLength);
    }

    let (pairs, remainder) = payload.as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    let mut bytes = [0_u8; TOKEN_BYTES];
    for (output, pair) in bytes.iter_mut().zip(pairs) {
        *output = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(bytes)
}

const fn hex_nibble(byte: u8) -> Result<u8, TokenParseError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(TokenParseError::InvalidEncoding),
    }
}

fn encode_token(prefix: &str, bytes: &[u8; TOKEN_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(prefix.len() + TOKEN_HEX_LEN);
    encoded.push_str(prefix);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO_MANAGEMENT: &str =
        "sym_mgmt_0000000000000000000000000000000000000000000000000000000000000000";
    const ZERO_CLAIM: &str =
        "sym_claim_0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn generated_tokens_have_256_bit_lowercase_hex_payloads() {
        let management = ManagementToken::generate().unwrap().encode();
        let claim = ClaimToken::generate().unwrap().encode();

        assert_valid_encoding(&management, MANAGEMENT_TOKEN_PREFIX);
        assert_valid_encoding(&claim, CLAIM_TOKEN_PREFIX);
        assert_ne!(management, ManagementToken::generate().unwrap().encode());
        assert_ne!(claim, ClaimToken::generate().unwrap().encode());
    }

    #[test]
    fn typed_tokens_round_trip() {
        let management = ManagementToken::parse(ZERO_MANAGEMENT).unwrap();
        let claim = ClaimToken::parse(ZERO_CLAIM).unwrap();

        assert_eq!(management.encode(), ZERO_MANAGEMENT);
        assert_eq!(claim.encode(), ZERO_CLAIM);
    }

    #[test]
    fn token_enum_dispatches_once_after_parsing() {
        let management = Token::parse(ZERO_MANAGEMENT).unwrap();
        let claim = Token::parse(ZERO_CLAIM).unwrap();

        assert_eq!(management.kind(), TokenKind::Management);
        assert_eq!(claim.kind(), TokenKind::Claim);
        assert!(matches!(management.hash(), TokenHash::Management(_)));
        assert!(matches!(claim.hash(), TokenHash::Claim(_)));
    }

    #[test]
    fn parser_rejects_wrong_prefix_length_and_encoding() {
        assert_eq!(
            ManagementToken::parse(ZERO_CLAIM).unwrap_err(),
            TokenParseError::UnknownPrefix
        );
        assert_eq!(
            ClaimToken::parse("sym_claim_00").unwrap_err(),
            TokenParseError::InvalidLength
        );
        let uppercase = format!(
            "{MANAGEMENT_TOKEN_PREFIX}A{}",
            &ZERO_MANAGEMENT[MANAGEMENT_TOKEN_PREFIX.len() + 1..]
        );
        assert_eq!(
            ManagementToken::parse(&uppercase).unwrap_err(),
            TokenParseError::InvalidEncoding
        );
        assert_eq!(
            Token::parse("other_0000").unwrap_err(),
            TokenParseError::UnknownPrefix
        );
    }

    #[test]
    fn hashes_verify_matching_tokens_and_reject_others() {
        let management = ManagementToken::parse(ZERO_MANAGEMENT).unwrap();
        let other = ManagementToken::generate().unwrap();
        let hash = management.hash();

        assert!(hash.verify(&management));
        assert!(!hash.verify(&other));

        let claim = ClaimToken::parse(ZERO_CLAIM).unwrap();
        let other = ClaimToken::generate().unwrap();
        let hash = claim.hash();
        assert!(hash.verify(&claim));
        assert!(!hash.verify(&other));
    }

    #[test]
    fn hash_domains_are_distinct_for_identical_payloads() {
        let management = ManagementToken::parse(ZERO_MANAGEMENT).unwrap();
        let claim = ClaimToken::parse(ZERO_CLAIM).unwrap();

        assert_ne!(management.hash().as_bytes(), claim.hash().as_bytes());
    }

    #[test]
    fn debug_output_never_contains_plaintext() {
        let management = ManagementToken::parse(ZERO_MANAGEMENT).unwrap();
        let claim = ClaimToken::parse(ZERO_CLAIM).unwrap();
        let management_debug = format!("{management:?}");
        let claim_debug = format!("{claim:?}");

        assert!(!management_debug.contains(&ZERO_MANAGEMENT[MANAGEMENT_TOKEN_PREFIX.len()..]));
        assert!(!claim_debug.contains(&ZERO_CLAIM[CLAIM_TOKEN_PREFIX.len()..]));
        assert!(management_debug.contains("REDACTED"));
        assert!(claim_debug.contains("REDACTED"));
        assert!(!format!("{:?}", management.hash()).contains("00000000"));
    }

    fn assert_valid_encoding(encoded: &str, prefix: &str) {
        let payload = encoded.strip_prefix(prefix).unwrap();
        assert_eq!(payload.len(), TOKEN_HEX_LEN);
        assert!(payload.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(payload.bytes().all(|byte| !byte.is_ascii_uppercase()));
    }
}
