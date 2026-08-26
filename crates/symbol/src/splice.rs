use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, header};
use futures_util::{Stream, StreamExt as _};
use tokio::io::AsyncWriteExt as _;

use crate::store::{Splice, SpliceSource};

pub const MAX_HEADER_BYTES: usize = symbol_contract::SPLICE_HEADER_MAX_BYTES;
pub const MAX_HEADER_DESCRIPTORS: usize = symbol_contract::SPLICE_HEADER_MAX_DESCRIPTORS;
pub const MAX_FRAME_DESCRIPTORS: usize = symbol_contract::SPLICE_FRAME_MAX_DESCRIPTORS;
pub const MAX_FRAME_METADATA_BYTES: usize = symbol_contract::SPLICE_FRAME_MAX_METADATA_BYTES;

const FRAME_MAGIC: &[u8; 8] = b"SYMSPL1\0";
const FRAME_PREFIX_BYTES: usize = 16;
const FRAME_DESCRIPTOR_BYTES: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("error: malformed Splice descriptor")]
    MalformedDescriptor,
    #[error("error: Splice descriptors are required")]
    MissingDescriptors,
    #[error("error: Splice header limits exceeded")]
    HeaderLimit,
    #[error("error: malformed Symbol splice frame")]
    MalformedFrame,
    #[error("error: Symbol splice frame limits exceeded")]
    FrameLimit,
    #[error("error: splice insertion payload is too large")]
    PayloadTooLarge,
    #[error("error: splice insertion payload length does not match its descriptors")]
    PayloadLength,
    #[error("error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Header,
    FrameV1,
}

#[derive(Debug)]
struct Descriptor {
    offset: u64,
    delete: u64,
    insert: u64,
}

#[derive(Debug)]
struct OwnedSplice {
    offset: u64,
    delete: u64,
    insert: Option<PathBuf>,
}

#[derive(Debug)]
pub struct ParsedSplices {
    directory: PathBuf,
    splices: Vec<OwnedSplice>,
}

impl ParsedSplices {
    pub const fn count(&self) -> usize {
        self.splices.len()
    }

    pub fn as_store_splices(&self) -> Vec<Splice<'_>> {
        self.splices
            .iter()
            .map(|splice| Splice {
                offset: splice.offset,
                delete: splice.delete,
                insert: splice
                    .insert
                    .as_deref()
                    .map_or(SpliceSource::Empty, SpliceSource::File),
            })
            .collect()
    }
}

impl Drop for ParsedSplices {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub fn select_format(headers: &HeaderMap) -> Result<Format, ProtocolError> {
    let has_header = headers.get_all("splice").iter().next().is_some();
    let framed = headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.to_str().is_ok_and(is_frame_content_type));
    match (has_header, framed) {
        (true, false) => Ok(Format::Header),
        (false, true) => Ok(Format::FrameV1),
        (true, true) => Err(ProtocolError::MalformedFrame),
        (false, false) => Err(ProtocolError::MissingDescriptors),
    }
}

pub async fn parse(
    format: Format,
    headers: &HeaderMap,
    body: Body,
    directory: PathBuf,
    maximum_insert_bytes: u64,
) -> Result<ParsedSplices, ProtocolError> {
    let temporary = TemporaryDirectory::create(directory)?;
    let mut reader = StreamReader::new(body.into_data_stream());
    let descriptors = match format {
        Format::Header => parse_header_descriptors(headers, maximum_insert_bytes)?,
        Format::FrameV1 => parse_frame_descriptors(&mut reader, maximum_insert_bytes).await?,
    };
    let splices = spool_insertions(
        &mut reader,
        &descriptors,
        temporary.path(),
        maximum_insert_bytes,
    )
    .await?;
    let directory = temporary.persist();
    Ok(ParsedSplices { directory, splices })
}

fn is_frame_content_type(value: &str) -> bool {
    let mut parts = value.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("application/vnd.symbol.splice"))
    {
        return false;
    }
    let mut version = None;
    for part in parts {
        let Some((name, value)) = part.split_once('=') else {
            return false;
        };
        if !name.trim().eq_ignore_ascii_case("version") || version.replace(value.trim()).is_some() {
            return false;
        }
    }
    version == Some("1")
}

fn parse_header_descriptors(
    headers: &HeaderMap,
    maximum_insert_bytes: u64,
) -> Result<Vec<Descriptor>, ProtocolError> {
    let values = headers.get_all("splice");
    let mut aggregate = String::new();
    for value in values {
        let value = value
            .to_str()
            .map_err(|_| ProtocolError::MalformedDescriptor)?;
        let separator_bytes = usize::from(!aggregate.is_empty());
        let combined_bytes = aggregate
            .len()
            .checked_add(separator_bytes)
            .and_then(|length| length.checked_add(value.len()))
            .ok_or(ProtocolError::HeaderLimit)?;
        if combined_bytes > MAX_HEADER_BYTES {
            return Err(ProtocolError::HeaderLimit);
        }
        if separator_bytes != 0 {
            aggregate.push(',');
        }
        aggregate.push_str(value);
    }
    if aggregate.is_empty() {
        return Err(ProtocolError::MissingDescriptors);
    }
    let mut descriptors = Vec::new();
    let mut total_insert = 0_u64;
    for encoded in aggregate.split(',') {
        if descriptors.len() == MAX_HEADER_DESCRIPTORS {
            return Err(ProtocolError::HeaderLimit);
        }
        let descriptor = parse_header_descriptor(encoded)?;
        total_insert = total_insert
            .checked_add(descriptor.insert)
            .ok_or(ProtocolError::PayloadTooLarge)?;
        if total_insert > maximum_insert_bytes {
            return Err(ProtocolError::PayloadTooLarge);
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

fn parse_header_descriptor(encoded: &str) -> Result<Descriptor, ProtocolError> {
    let mut offset = None;
    let mut delete = None;
    let mut insert = None;
    for field in encoded.split(';') {
        let (name, value) = field
            .trim()
            .split_once('=')
            .ok_or(ProtocolError::MalformedDescriptor)?;
        let value = value
            .trim()
            .parse::<u64>()
            .map_err(|_| ProtocolError::MalformedDescriptor)?;
        match name.trim() {
            "offset" if offset.replace(value).is_none() => {}
            "delete" if delete.replace(value).is_none() => {}
            "insert" if insert.replace(value).is_none() => {}
            _ => return Err(ProtocolError::MalformedDescriptor),
        }
    }
    Ok(Descriptor {
        offset: offset.ok_or(ProtocolError::MalformedDescriptor)?,
        delete: delete.ok_or(ProtocolError::MalformedDescriptor)?,
        insert: insert.ok_or(ProtocolError::MalformedDescriptor)?,
    })
}

async fn parse_frame_descriptors<S, E>(
    reader: &mut StreamReader<S>,
    maximum_insert_bytes: u64,
) -> Result<Vec<Descriptor>, ProtocolError>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::fmt::Display,
{
    let prefix = reader
        .read_exact(FRAME_PREFIX_BYTES)
        .await
        .map_err(frame_read_error)?;
    if &prefix[..8] != FRAME_MAGIC {
        return Err(ProtocolError::MalformedFrame);
    }
    let count = u32::from_be_bytes(
        prefix[8..12]
            .try_into()
            .expect("frame count occupies four bytes"),
    ) as usize;
    let flags = u32::from_be_bytes(
        prefix[12..16]
            .try_into()
            .expect("frame flags occupy four bytes"),
    );
    let metadata_bytes = count
        .checked_mul(FRAME_DESCRIPTOR_BYTES)
        .ok_or(ProtocolError::FrameLimit)?;
    if count == 0
        || count > MAX_FRAME_DESCRIPTORS
        || metadata_bytes > MAX_FRAME_METADATA_BYTES
        || flags != 0
    {
        return Err(if flags == 0 {
            ProtocolError::FrameLimit
        } else {
            ProtocolError::MalformedFrame
        });
    }
    let table = reader
        .read_exact(metadata_bytes)
        .await
        .map_err(frame_read_error)?;
    let mut descriptors = Vec::with_capacity(count);
    let mut total_insert = 0_u64;
    for row in table.as_chunks::<FRAME_DESCRIPTOR_BYTES>().0 {
        let offset = u64::from_be_bytes(row[0..8].try_into().expect("eight-byte offset"));
        let delete = u64::from_be_bytes(row[8..16].try_into().expect("eight-byte deletion"));
        let insert = u64::from_be_bytes(row[16..24].try_into().expect("eight-byte insertion"));
        total_insert = total_insert
            .checked_add(insert)
            .ok_or(ProtocolError::PayloadTooLarge)?;
        if total_insert > maximum_insert_bytes {
            return Err(ProtocolError::PayloadTooLarge);
        }
        descriptors.push(Descriptor {
            offset,
            delete,
            insert,
        });
    }
    Ok(descriptors)
}

fn frame_read_error(error: ProtocolError) -> ProtocolError {
    match error {
        ProtocolError::PayloadLength => ProtocolError::MalformedFrame,
        other => other,
    }
}

async fn spool_insertions<S, E>(
    reader: &mut StreamReader<S>,
    descriptors: &[Descriptor],
    directory: &Path,
    maximum_insert_bytes: u64,
) -> Result<Vec<OwnedSplice>, ProtocolError>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::fmt::Display,
{
    let mut total = 0_u64;
    let mut splices = Vec::with_capacity(descriptors.len());
    for (index, descriptor) in descriptors.iter().enumerate() {
        total = total
            .checked_add(descriptor.insert)
            .ok_or(ProtocolError::PayloadTooLarge)?;
        if total > maximum_insert_bytes {
            return Err(ProtocolError::PayloadTooLarge);
        }
        let insert = if descriptor.insert == 0 {
            None
        } else {
            let path = directory.join(format!("insert-{index}"));
            let mut file = tokio::fs::File::create(&path).await?;
            reader.copy_exact(&mut file, descriptor.insert).await?;
            file.sync_all().await?;
            Some(path)
        };
        splices.push(OwnedSplice {
            offset: descriptor.offset,
            delete: descriptor.delete,
            insert,
        });
    }
    reader.ensure_eof().await?;
    Ok(splices)
}

struct TemporaryDirectory {
    path: Option<PathBuf>,
}

impl TemporaryDirectory {
    fn create(path: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&path)?;
        Ok(Self { path: Some(path) })
    }

    fn path(&self) -> &Path {
        self.path.as_deref().expect("temporary directory exists")
    }

    fn persist(mut self) -> PathBuf {
        self.path.take().expect("temporary directory exists")
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

struct StreamReader<S> {
    stream: Pin<Box<S>>,
    pending: Bytes,
}

impl<S> StreamReader<S> {
    fn new(stream: S) -> Self {
        Self {
            stream: Box::pin(stream),
            pending: Bytes::new(),
        }
    }
}

impl<S, E> StreamReader<S>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::fmt::Display,
{
    async fn read_exact(&mut self, length: usize) -> Result<Vec<u8>, ProtocolError> {
        let mut output = Vec::with_capacity(length);
        while output.len() < length {
            self.refill().await?;
            if self.pending.is_empty() {
                return Err(ProtocolError::PayloadLength);
            }
            let take = (length - output.len()).min(self.pending.len());
            output.extend_from_slice(&self.pending.split_to(take));
        }
        Ok(output)
    }

    async fn copy_exact(
        &mut self,
        output: &mut tokio::fs::File,
        mut length: u64,
    ) -> Result<(), ProtocolError> {
        while length > 0 {
            self.refill().await?;
            if self.pending.is_empty() {
                return Err(ProtocolError::PayloadLength);
            }
            let take = usize::try_from(length)
                .unwrap_or(usize::MAX)
                .min(self.pending.len());
            output.write_all(&self.pending[..take]).await?;
            self.pending.advance(take);
            length -= u64::try_from(take).expect("chunk length fits in u64");
        }
        Ok(())
    }

    async fn ensure_eof(&mut self) -> Result<(), ProtocolError> {
        if !self.pending.is_empty() {
            return Err(ProtocolError::PayloadLength);
        }
        loop {
            match self.stream.next().await {
                Some(Ok(chunk)) if chunk.is_empty() => {}
                Some(Ok(_)) => return Err(ProtocolError::PayloadLength),
                Some(Err(error)) => return Err(io::Error::other(error.to_string()).into()),
                None => return Ok(()),
            }
        }
    }

    async fn refill(&mut self) -> Result<(), ProtocolError> {
        while self.pending.is_empty() {
            match self.stream.next().await {
                Some(Ok(chunk)) => self.pending = chunk,
                Some(Err(error)) => return Err(io::Error::other(error.to_string()).into()),
                None => return Ok(()),
            }
        }
        Ok(())
    }
}

trait BytesAdvance {
    fn advance(&mut self, count: usize);
}

impl BytesAdvance for Bytes {
    fn advance(&mut self, count: usize) {
        *self = self.split_off(count);
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn frame(descriptors: &[(u64, u64, u64)], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(FRAME_MAGIC);
        bytes.extend_from_slice(
            &u32::try_from(descriptors.len())
                .expect("test descriptor count fits")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        for (offset, delete, insert) in descriptors {
            bytes.extend_from_slice(&offset.to_be_bytes());
            bytes.extend_from_slice(&delete.to_be_bytes());
            bytes.extend_from_slice(&insert.to_be_bytes());
        }
        bytes.extend_from_slice(payload);
        bytes
    }

    #[tokio::test]
    async fn repeated_and_combined_headers_stream_exact_insertions() {
        let root = tempfile::tempdir().unwrap();
        let mut headers = HeaderMap::new();
        headers.append(
            "splice",
            HeaderValue::from_static("offset=0; delete=1; insert=2"),
        );
        headers.append(
            "splice",
            HeaderValue::from_static("offset=3; delete=0; insert=1, offset=5; delete=0; insert=0"),
        );
        let parsed = parse(
            Format::Header,
            &headers,
            Body::from("abc"),
            root.path().join("splices"),
            1024,
        )
        .await
        .unwrap();
        assert_eq!(parsed.count(), 3);
        let splices = parsed.as_store_splices();
        let SpliceSource::File(first) = splices[0].insert else {
            panic!("first insertion must be spooled");
        };
        let SpliceSource::File(second) = splices[1].insert else {
            panic!("second insertion must be spooled");
        };
        assert_eq!(std::fs::read(first).unwrap(), b"ab");
        assert_eq!(std::fs::read(second).unwrap(), b"c");
        assert!(matches!(splices[2].insert, SpliceSource::Empty));
    }

    #[test]
    fn header_limits_are_exact() {
        let descriptor = "offset=0; delete=0; insert=0";
        let mut headers = HeaderMap::new();
        headers.insert(
            "splice",
            HeaderValue::from_str(&vec![descriptor; MAX_HEADER_DESCRIPTORS - 1].join(",")).unwrap(),
        );
        assert_eq!(
            parse_header_descriptors(&headers, 0).unwrap().len(),
            MAX_HEADER_DESCRIPTORS - 1
        );
        headers.insert(
            "splice",
            HeaderValue::from_str(&vec![descriptor; MAX_HEADER_DESCRIPTORS].join(",")).unwrap(),
        );
        assert_eq!(
            parse_header_descriptors(&headers, 0).unwrap().len(),
            MAX_HEADER_DESCRIPTORS
        );

        headers.insert(
            "splice",
            HeaderValue::from_str(&vec![descriptor; MAX_HEADER_DESCRIPTORS + 1].join(",")).unwrap(),
        );
        assert!(matches!(
            parse_header_descriptors(&headers, 0),
            Err(ProtocolError::HeaderLimit)
        ));

        let boundary = format!(
            "{descriptor}{}",
            " ".repeat(MAX_HEADER_BYTES - descriptor.len())
        );
        headers.insert("splice", HeaderValue::from_str(&boundary).unwrap());
        assert_eq!(parse_header_descriptors(&headers, 0).unwrap().len(), 1);

        let oversized = format!("{boundary} ");
        headers.insert("splice", HeaderValue::from_str(&oversized).unwrap());
        assert!(matches!(
            parse_header_descriptors(&headers, 0),
            Err(ProtocolError::HeaderLimit)
        ));

        let first = format!(
            "{descriptor}{}",
            " ".repeat(MAX_HEADER_BYTES - (2 * descriptor.len()) - 1)
        );
        let mut repeated = HeaderMap::new();
        repeated.append("splice", HeaderValue::from_str(&first).unwrap());
        repeated.append("splice", HeaderValue::from_str(descriptor).unwrap());
        assert_eq!(
            first.len() + 1 + descriptor.len(),
            MAX_HEADER_BYTES,
            "combined boundary includes the inserted comma"
        );
        assert_eq!(parse_header_descriptors(&repeated, 0).unwrap().len(), 2);

        let oversized_first = format!("{first} ");
        let mut repeated_oversized = HeaderMap::new();
        repeated_oversized.append("splice", HeaderValue::from_str(&oversized_first).unwrap());
        repeated_oversized.append("splice", HeaderValue::from_str(descriptor).unwrap());
        assert_eq!(
            oversized_first.len() + 1 + descriptor.len(),
            MAX_HEADER_BYTES + 1
        );
        assert!(matches!(
            parse_header_descriptors(&repeated_oversized, 0),
            Err(ProtocolError::HeaderLimit)
        ));
    }

    #[tokio::test]
    async fn frame_v1_validates_magic_flags_counts_lengths_and_trailing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let headers = HeaderMap::new();
        let parsed = parse(
            Format::FrameV1,
            &headers,
            Body::from(frame(&[(0, 0, 2), (2, 1, 1)], b"abc")),
            root.path().join("valid"),
            1024,
        )
        .await
        .unwrap();
        assert_eq!(parsed.count(), 2);

        for count in [MAX_FRAME_DESCRIPTORS - 1, MAX_FRAME_DESCRIPTORS] {
            let descriptors = vec![(0, 0, 0); count];
            let parsed = parse(
                Format::FrameV1,
                &headers,
                Body::from(frame(&descriptors, b"")),
                root.path().join(format!("boundary-{count}")),
                1024,
            )
            .await
            .unwrap();
            assert_eq!(parsed.count(), count);
        }

        let mut flags = frame(&[(0, 0, 0)], b"");
        flags[15] = 1;
        assert!(matches!(
            parse(
                Format::FrameV1,
                &headers,
                Body::from(flags),
                root.path().join("flags"),
                1024,
            )
            .await,
            Err(ProtocolError::MalformedFrame)
        ));

        let mut count = Vec::from(FRAME_MAGIC.as_slice());
        count.extend_from_slice(
            &u32::try_from(MAX_FRAME_DESCRIPTORS + 1)
                .unwrap()
                .to_be_bytes(),
        );
        count.extend_from_slice(&0_u32.to_be_bytes());
        assert!(matches!(
            parse(
                Format::FrameV1,
                &headers,
                Body::from(count),
                root.path().join("count"),
                1024,
            )
            .await,
            Err(ProtocolError::FrameLimit)
        ));

        assert!(matches!(
            parse(
                Format::FrameV1,
                &headers,
                Body::from(frame(&[(0, 0, 2)], b"x")),
                root.path().join("short"),
                1024,
            )
            .await,
            Err(ProtocolError::PayloadLength)
        ));
        assert!(matches!(
            parse(
                Format::FrameV1,
                &headers,
                Body::from(frame(&[(0, 0, 1)], b"xy")),
                root.path().join("trailing"),
                1024,
            )
            .await,
            Err(ProtocolError::PayloadLength)
        ));
    }
}
