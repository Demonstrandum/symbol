use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;

use diesel::deserialize::{self, FromSql, Queryable};
use diesel::expression::AsExpression;
use diesel::serialize::{self, Output, ToSql};
use diesel::sql_types::Binary;
use diesel::sqlite::Sqlite;

pub const HASH_BYTES: usize = 32;
pub const HASH_HEX_LEN: usize = HASH_BYTES * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, AsExpression)]
#[diesel(sql_type = Binary)]
pub struct ContentHash([u8; HASH_BYTES]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, AsExpression)]
#[diesel(sql_type = Binary)]
pub struct TreeHash([u8; HASH_BYTES]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HashParseError {
    #[error("hash must be exactly {HASH_HEX_LEN} hexadecimal characters")]
    InvalidLength,
    #[error("hash is not hexadecimal")]
    InvalidEncoding,
}

impl ContentHash {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; HASH_BYTES]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; HASH_BYTES] {
        &self.0
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, HashParseError> {
        let bytes: [u8; HASH_BYTES] = bytes
            .try_into()
            .map_err(|_| HashParseError::InvalidLength)?;
        Ok(Self(bytes))
    }

    pub fn parse_hex(value: &str) -> Result<Self, HashParseError> {
        parse_hex(value).map(Self)
    }

    pub fn parse_wire(value: &str) -> Result<Self, HashParseError> {
        let value = value.strip_prefix("blake3:").unwrap_or(value);
        Self::parse_hex(value)
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

impl TreeHash {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; HASH_BYTES]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; HASH_BYTES] {
        &self.0
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, HashParseError> {
        let bytes: [u8; HASH_BYTES] = bytes
            .try_into()
            .map_err(|_| HashParseError::InvalidLength)?;
        Ok(Self(bytes))
    }

    pub fn parse_wire(value: &str) -> Result<Self, HashParseError> {
        let value = value.strip_prefix("blake3:").unwrap_or(value);
        if value.is_empty() {
            return Ok(Self::default());
        }
        parse_hex(value).map(Self)
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    #[must_use]
    pub fn to_wire(self) -> String {
        if self.0 == [0; HASH_BYTES] {
            String::new()
        } else {
            format!("blake3:{}", self.to_hex())
        }
    }
}

impl From<[u8; HASH_BYTES]> for ContentHash {
    fn from(bytes: [u8; HASH_BYTES]) -> Self {
        Self(bytes)
    }
}

impl From<[u8; HASH_BYTES]> for TreeHash {
    fn from(bytes: [u8; HASH_BYTES]) -> Self {
        Self(bytes)
    }
}

impl From<blake3::Hash> for ContentHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }
}

impl From<blake3::Hash> for TreeHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }
}

impl From<ContentHash> for [u8; HASH_BYTES] {
    fn from(hash: ContentHash) -> Self {
        hash.0
    }
}

impl From<TreeHash> for [u8; HASH_BYTES] {
    fn from(hash: TreeHash) -> Self {
        hash.0
    }
}

impl AsRef<[u8]> for ContentHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for TreeHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> Self {
        hash.to_hex()
    }
}

impl From<TreeHash> for String {
    fn from(hash: TreeHash) -> Self {
        hash.to_wire()
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl fmt::Display for TreeHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

impl TryFrom<&str> for ContentHash {
    type Error = HashParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse_wire(value)
    }
}

impl TryFrom<String> for ContentHash {
    type Error = HashParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_wire(&value)
    }
}

impl TryFrom<&str> for TreeHash {
    type Error = HashParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse_wire(value)
    }
}

impl TryFrom<String> for TreeHash {
    type Error = HashParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_wire(&value)
    }
}

impl FromStr for ContentHash {
    type Err = HashParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_wire(value)
    }
}

impl FromStr for TreeHash {
    type Err = HashParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_wire(value)
    }
}

impl ToSql<Binary, Sqlite> for ContentHash {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        out.set_value(&self.0[..]);
        Ok(serialize::IsNull::No)
    }
}

impl FromSql<Binary, Sqlite> for ContentHash {
    fn from_sql(
        value: <Sqlite as diesel::backend::Backend>::RawValue<'_>,
    ) -> deserialize::Result<Self> {
        let bytes = <Vec<u8> as FromSql<Binary, Sqlite>>::from_sql(value)?;
        Self::from_slice(&bytes).map_err(|error| error.to_string().into())
    }
}

impl Queryable<Binary, Sqlite> for ContentHash {
    type Row = <Vec<u8> as Queryable<Binary, Sqlite>>::Row;

    fn build(row: Self::Row) -> deserialize::Result<Self> {
        let bytes = <Vec<u8> as Queryable<Binary, Sqlite>>::build(row)?;
        Self::from_slice(&bytes).map_err(|error| error.to_string().into())
    }
}

impl ToSql<Binary, Sqlite> for TreeHash {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        out.set_value(&self.0[..]);
        Ok(serialize::IsNull::No)
    }
}

impl FromSql<Binary, Sqlite> for TreeHash {
    fn from_sql(
        value: <Sqlite as diesel::backend::Backend>::RawValue<'_>,
    ) -> deserialize::Result<Self> {
        let bytes = <Vec<u8> as FromSql<Binary, Sqlite>>::from_sql(value)?;
        Self::from_slice(&bytes).map_err(|error| error.to_string().into())
    }
}

impl Queryable<Binary, Sqlite> for TreeHash {
    type Row = <Vec<u8> as Queryable<Binary, Sqlite>>::Row;

    fn build(row: Self::Row) -> deserialize::Result<Self> {
        let bytes = <Vec<u8> as Queryable<Binary, Sqlite>>::build(row)?;
        Self::from_slice(&bytes).map_err(|error| error.to_string().into())
    }
}

fn encode_hex(bytes: &[u8; HASH_BYTES]) -> String {
    let mut hex = String::with_capacity(HASH_HEX_LEN);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

fn parse_hex(value: &str) -> Result<[u8; HASH_BYTES], HashParseError> {
    if value.len() != HASH_HEX_LEN {
        return Err(HashParseError::InvalidLength);
    }
    let (pairs, rest) = value.as_bytes().as_chunks::<2>();
    debug_assert!(rest.is_empty(), "{HASH_HEX_LEN} is even");
    let mut out = [0_u8; HASH_BYTES];
    for (slot, pair) in out.iter_mut().zip(pairs) {
        let hi = hex_value(pair[0]).ok_or(HashParseError::InvalidEncoding)?;
        let lo = hex_value(pair[1]).ok_or(HashParseError::InvalidEncoding)?;
        *slot = (hi << 4) | lo;
    }
    Ok(out)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_accepts_wire_and_bare_hex() {
        let digest = blake3::hash(b"payload");
        let hash = ContentHash::from(digest);
        let wire = format!("blake3:{}", digest.to_hex());
        assert_eq!(ContentHash::try_from(wire.as_str()).unwrap(), hash);
        assert_eq!(ContentHash::try_from(digest.to_hex().as_str()).unwrap(), hash);
    }

    #[test]
    fn content_hash_round_trips_hex() {
        let digest = blake3::hash(b"payload");
        let hash = ContentHash::from(digest);
        assert_eq!(hash.to_string(), digest.to_hex().to_string());
        assert_eq!(ContentHash::try_from(hash.to_hex().as_str()).unwrap(), hash);
    }

    #[test]
    fn tree_hash_accepts_wire_and_bare_hex() {
        let digest = blake3::hash(b"tree");
        let hash = TreeHash::from(digest);
        let wire = format!("blake3:{}", digest.to_hex());
        assert_eq!(TreeHash::try_from(wire.as_str()).unwrap(), hash);
        assert_eq!(TreeHash::try_from(digest.to_hex().as_str()).unwrap(), hash);
        assert_eq!(hash.to_string(), wire);
    }
}
