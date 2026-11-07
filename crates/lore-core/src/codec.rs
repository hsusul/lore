//! Lossless zstd compression of stored bodies, with one decoding boundary.
//!
//! A compressed value is a BLOB: one [`MAGIC`] byte, the id of the
//! dictionary it was compressed with as a little-endian `u32` (`0` for none),
//! then a zstd frame that carries its content size and a checksum. A plain
//! value stays TEXT, so both kinds coexist in one column and a reader branches
//! on the value's type.
//!
//! Four columns may hold compressed values ([`COLUMNS`]): `part_body.body`,
//! `message_part.text`, `message_part.content_json` and `tool_call.input_json`.
//! `search_document.redacted_text`, the full-text index's content, never does.
//!
//! The decoder is reached only through SQL: [`register`] installs two scalar
//! functions on every archive connection ([`crate::storage`] and
//! [`crate::storage::migrations`] call it), and the views of migration 0021
//! (`part_body_full`, `message_part_full`, `tool_call_full`) hold the one
//! expression per column that applies them ([`unpack_sql`]):
//!
//! ```sql
//! lore_unpack(body, (SELECT dict FROM zstd_dict WHERE id = lore_dict_id(body)))
//! ```
//!
//! `lore_dict_id` reads a value's dictionary id (NULL for plain text), and
//! `lore_unpack` returns plain text unchanged and decompresses a BLOB with the
//! dictionary passed beside it. Passing the dictionary in, rather than looking
//! it up from inside the function, keeps the decode inside the reader's own
//! transaction snapshot with no state shared across connections. Every reader
//! selects from those views. A test (`tests/zstd_bodies.rs`) fails when other
//! SQL reads a raw column.
//!
//! A value that does not decode, because its header, frame, checksum or
//! dictionary is wrong, or because its bytes are not UTF-8, is a query error.
//! It is never a panic and never different bytes.
//!
//! A connection without the functions, such as the `sqlite3` shell, cannot
//! read the views and sees compressed values as BLOBs.

use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::ValueRef;
use rusqlite::Connection;

/// First byte of a compressed value. Never the first byte of UTF-8 text.
pub const MAGIC: u8 = 0xFF;

/// Header bytes before the zstd frame: [`MAGIC`] and the dictionary id.
pub const HEADER_LEN: usize = 5;

/// The zstd level bodies are compressed at. Level 3 is zstd's default: it
/// compresses at hundreds of MB/s with almost the ratio of much slower levels.
pub const LEVEL: i32 = 3;

/// Largest body a value may claim to decompress to: SQLite's default maximum
/// string length. A corrupt header cannot make the decoder allocate more.
pub const MAX_DECODED: usize = 1_000_000_000;

/// Values shorter than this many bytes are never compressed: the header and
/// frame overhead would eat most of the gain.
pub const MIN_LEN: usize = 64;

/// A column that may hold compressed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// The `zstd_dict.column` value naming this column's dictionaries.
    pub key: &'static str,
    pub table: &'static str,
    pub column: &'static str,
}

/// Attachment bodies, stored once each (migration 0019).
pub const PART_BODY: Column = Column {
    key: "part_body.body",
    table: "part_body",
    column: "body",
};

/// A part's text: prose, thinking, tool output.
pub const PART_TEXT: Column = Column {
    key: "message_part.text",
    table: "message_part",
    column: "text",
};

/// A part's raw payload, stored slim for tool results (migration 0020).
pub const PART_CONTENT: Column = Column {
    key: "message_part.content_json",
    table: "message_part",
    column: "content_json",
};

/// A tool call's input.
pub const TOOL_INPUT: Column = Column {
    key: "tool_call.input_json",
    table: "tool_call",
    column: "input_json",
};

/// Every column that may hold compressed values.
pub const COLUMNS: [Column; 4] = [PART_BODY, PART_TEXT, PART_CONTENT, TOOL_INPUT];

/// The SQL expression that decodes `expr`, a stored value of a compressed
/// column. The views hold one per column; compaction uses it to read a
/// rewritten value back through SQL.
#[must_use]
pub fn unpack_sql(expr: &str) -> String {
    format!(
        "lore_unpack({expr}, (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id({expr})))"
    )
}

/// Why a value did not decode. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("compressed value has a bad header")]
    Header,
    #[error("compressed value needs a dictionary that is missing")]
    MissingDictionary,
    #[error("compressed value claims an impossible size")]
    Size,
    #[error("compressed value does not decompress")]
    Frame,
    #[error("compressed value is not UTF-8 text")]
    NotText,
    #[error("value is neither text nor a compressed body")]
    Type,
}

/// The dictionary id in a compressed value's header, or `None` when `value`
/// is not a compressed value.
#[must_use]
pub fn dict_id(value: &[u8]) -> Option<u32> {
    match value {
        [MAGIC, a, b, c, d, ..] => Some(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => None,
    }
}

/// A compressor for one dictionary (or none), reused across rows.
pub struct Encoder {
    id: u32,
    inner: zstd::bulk::Compressor<'static>,
}

impl Encoder {
    /// An encoder for dictionary `id` holding `dict`, or for no dictionary
    /// when `dict` is `None` (then `id` is ignored and `0` is written).
    pub fn new(id: u32, dict: Option<&[u8]>) -> std::io::Result<Self> {
        let (id, mut inner) = match dict {
            Some(dict) => (id, zstd::bulk::Compressor::with_dictionary(LEVEL, dict)?),
            None => (0, zstd::bulk::Compressor::new(LEVEL)?),
        };
        inner.include_checksum(true)?;
        inner.include_contentsize(true)?;
        inner.include_dictid(true)?;
        Ok(Self { id, inner })
    }

    /// The compressed value for `plain`, or `None` when it would not be
    /// smaller than the text it replaces.
    pub fn encode(&mut self, plain: &str) -> Option<Vec<u8>> {
        let frame = self.inner.compress(plain.as_bytes()).ok()?;
        if HEADER_LEN + frame.len() >= plain.len() {
            return None;
        }
        let mut out = Vec::with_capacity(HEADER_LEN + frame.len());
        out.push(MAGIC);
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&frame);
        Some(out)
    }
}

/// A prepared decoder dictionary and the bytes it was built from.
struct Prepared {
    bytes: Vec<u8>,
    dict: zstd::dict::DecoderDictionary<'static>,
}

/// What `lore_unpack` keeps between rows on one connection: the prepared
/// dictionary, and the last two values it decoded. A view refers to a decoded
/// column more than once (the tool output marker tests and rebuilds
/// `content_json` from `text`), and SQLite evaluates the function at each
/// reference; the memo makes the repeats a compare instead of a decompress.
#[derive(Default)]
struct UnpackState {
    prepared: Option<Prepared>,
    memo: [Option<(Vec<u8>, String)>; 2],
    next: usize,
}

impl UnpackState {
    fn unpack(&mut self, value: &[u8], dict: Option<&[u8]>) -> Result<String, DecodeError> {
        let id = dict_id(value).ok_or(DecodeError::Header)?;
        if id != 0 {
            let dict = dict.ok_or(DecodeError::MissingDictionary)?;
            if self.prepared.as_ref().is_none_or(|p| p.bytes != dict) {
                // A different dictionary: the memo was decoded with the old one.
                self.memo = [None, None];
            }
        }
        if let Some((_, out)) = self.memo.iter().flatten().find(|(v, _)| v == value) {
            return Ok(out.clone());
        }
        let out = decode_blob(value, dict, &mut self.prepared)?;
        self.memo[self.next] = Some((value.to_vec(), out.clone()));
        self.next = (self.next + 1) % 2;
        Ok(out)
    }
}

/// Decompress the compressed value `value` with `dict`, the bytes of the
/// dictionary its header names (`None` when it names none or it is missing).
/// `cache` keeps the prepared dictionary between rows.
fn decode_blob(
    value: &[u8],
    dict: Option<&[u8]>,
    cache: &mut Option<Prepared>,
) -> Result<String, DecodeError> {
    let id = dict_id(value).ok_or(DecodeError::Header)?;
    let frame = value.get(HEADER_LEN..).ok_or(DecodeError::Header)?;
    let size = zstd::zstd_safe::get_frame_content_size(frame)
        .map_err(|_| DecodeError::Frame)?
        .ok_or(DecodeError::Size)?;
    let size = usize::try_from(size).map_err(|_| DecodeError::Size)?;
    if size > MAX_DECODED {
        return Err(DecodeError::Size);
    }
    let bytes = match (id, dict) {
        (0, _) => zstd::bulk::Decompressor::new()
            .and_then(|mut d| d.decompress(frame, size))
            .map_err(|_| DecodeError::Frame)?,
        (_, None) => return Err(DecodeError::MissingDictionary),
        (_, Some(dict)) => {
            if cache.as_ref().is_none_or(|p| p.bytes != dict) {
                *cache = Some(Prepared {
                    bytes: dict.to_vec(),
                    dict: zstd::dict::DecoderDictionary::copy(dict),
                });
            }
            let prepared = cache.as_ref().ok_or(DecodeError::MissingDictionary)?;
            zstd::bulk::Decompressor::with_prepared_dictionary(&prepared.dict)
                .and_then(|mut d| d.decompress(frame, size))
                .map_err(|_| DecodeError::Frame)?
        }
    };
    if bytes.len() != size {
        return Err(DecodeError::Frame);
    }
    String::from_utf8(bytes).map_err(|_| DecodeError::NotText)
}

/// Decode a stored value: TEXT as it is, a compressed BLOB decompressed with
/// `dict`, NULL as `None`. For tests and the guard that SQL does the same.
pub fn decode(value: ValueRef<'_>, dict: Option<&[u8]>) -> Result<Option<String>, DecodeError> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(|s| Some(s.to_owned()))
            .map_err(|_| DecodeError::NotText),
        ValueRef::Blob(b) => decode_blob(b, dict, &mut None).map(Some),
        _ => Err(DecodeError::Type),
    }
}

fn user_error(e: DecodeError) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(e))
}

/// Install `lore_dict_id(value)` and `lore_unpack(value, dict)` on `conn`.
/// Both are deterministic and innocuous, so the schema's views may call them.
pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("lore_dict_id", 1, flags, |ctx: &Context<'_>| {
        Ok(match ctx.get_raw(0) {
            ValueRef::Blob(b) => dict_id(b).map(i64::from),
            _ => None,
        })
    })?;
    // The state sits behind a mutex only because SQLite requires the function
    // to be `Send`; one connection calls it from one thread at a time.
    let state: Arc<Mutex<UnpackState>> = Arc::new(Mutex::new(UnpackState::default()));
    conn.create_scalar_function("lore_unpack", 2, flags, move |ctx: &Context<'_>| {
        let dict = match ctx.get_raw(1) {
            ValueRef::Blob(d) => Some(d),
            ValueRef::Null => None,
            _ => return Err(user_error(DecodeError::Type)),
        };
        match ctx.get_raw(0) {
            ValueRef::Null => Ok(None),
            ValueRef::Text(t) => std::str::from_utf8(t)
                .map(|s| Some(s.to_owned()))
                .map_err(|_| user_error(DecodeError::NotText)),
            ValueRef::Blob(b) => {
                let mut state = state
                    .lock()
                    .map_err(|_| user_error(DecodeError::MissingDictionary))?;
                state.unpack(b, dict).map(Some).map_err(user_error)
            }
            _ => Err(user_error(DecodeError::Type)),
        }
    })?;
    Ok(())
}

/// Train a dictionary of at most `max_size` bytes from `samples`. `None` when
/// zstd cannot train one from them (too few or too uniform).
#[must_use]
pub fn train(samples: &[Vec<u8>], max_size: usize) -> Option<Vec<u8>> {
    zstd::dict::from_samples(samples, max_size).ok()
}

/// The newest dictionary for `column`: its id and bytes.
pub fn current_dict(
    conn: &Connection,
    column: &Column,
) -> rusqlite::Result<Option<(u32, Vec<u8>)>> {
    use rusqlite::OptionalExtension;
    conn.prepare_cached(
        "SELECT id, dict FROM zstd_dict WHERE \"column\" = ?1 ORDER BY id DESC LIMIT 1",
    )?
    .query_row([column.key], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
}

/// A dictionary as writers use it: its id, bytes and a digest of the bytes.
#[derive(Debug, Clone)]
pub struct Dict {
    pub id: u32,
    pub bytes: Vec<u8>,
    digest: blake3::Hash,
}

impl Dict {
    #[must_use]
    pub fn new(id: u32, bytes: Vec<u8>) -> Self {
        let digest = blake3::hash(&bytes);
        Self { id, bytes, digest }
    }
}

/// An encoder and a prepared decoder for one dictionary.
struct Codec {
    id: u32,
    digest: blake3::Hash,
    encoder: Encoder,
    decoder: zstd::dict::DecoderDictionary<'static>,
}

/// Most dictionaries [`codecs`] keeps: four columns, with room for an archive
/// or two more in one process.
const CODECS_KEPT: usize = 16;

/// Codecs shared by every writer in the process, keyed by dictionary id and
/// digest (two archives in one process can each have a dictionary 1).
/// Building one digests the whole dictionary, which is too slow to repeat for
/// every session an ingest writes or every row compaction converts.
fn codecs() -> &'static Mutex<Vec<Codec>> {
    static CODECS: OnceLock<Mutex<Vec<Codec>>> = OnceLock::new();
    CODECS.get_or_init(|| Mutex::new(Vec::new()))
}

/// What [`pack_checked`] made of a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packed {
    /// Compressed, and decompressed back to the exact bytes.
    Value(Vec<u8>),
    /// Too short, or would not shrink: stored plain.
    Plain,
    /// The encoder failed or the result did not decompress to the exact
    /// bytes: stored plain, and counted by compaction.
    Failed,
}

/// Compress `plain` with `dict`, checked: the result is decompressed and
/// compared with `plain` before it is returned.
pub fn pack_checked(dict: &Dict, plain: &str) -> Packed {
    if plain.len() < MIN_LEN {
        return Packed::Plain;
    }
    let Ok(mut cache) = codecs().lock() else {
        return Packed::Failed;
    };
    let found = cache
        .iter()
        .position(|c| c.id == dict.id && c.digest == dict.digest);
    let at = match found {
        Some(at) => at,
        None => match Encoder::new(dict.id, Some(&dict.bytes)) {
            Ok(encoder) => {
                if cache.len() >= CODECS_KEPT {
                    cache.clear();
                }
                cache.push(Codec {
                    id: dict.id,
                    digest: dict.digest,
                    encoder,
                    decoder: zstd::dict::DecoderDictionary::copy(&dict.bytes),
                });
                cache.len() - 1
            }
            Err(_) => return Packed::Failed,
        },
    };
    let codec = &mut cache[at];
    let Some(packed) = codec.encoder.encode(plain) else {
        return Packed::Plain;
    };
    let back = packed
        .get(HEADER_LEN..)
        .ok_or(DecodeError::Header)
        .and_then(|frame| {
            zstd::bulk::Decompressor::with_prepared_dictionary(&codec.decoder)
                .and_then(|mut d| d.decompress(frame, plain.len()))
                .map_err(|_| DecodeError::Frame)
        });
    match back {
        Ok(back) if back == plain.as_bytes() => Packed::Value(packed),
        _ => Packed::Failed,
    }
}

/// Writes values of the compressed columns: compressed once the column has a
/// dictionary, plain until then. Loaded per transaction, so a dictionary
/// committed by compaction applies to the next write.
pub struct Packer {
    dicts: Vec<(Column, Dict)>,
}

impl Packer {
    /// The current dictionary of every column, read in `conn`'s transaction.
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let mut dicts = Vec::new();
        for column in COLUMNS {
            if let Some((id, dict)) = current_dict(conn, &column)? {
                dicts.push((column, Dict::new(id, dict)));
            }
        }
        Ok(Self { dicts })
    }

    /// The value to store for `plain` in `column`: a compressed BLOB when the
    /// column has a dictionary and the value shrinks, else the text itself.
    #[must_use]
    pub fn pack(&self, column: &Column, plain: &str) -> rusqlite::types::Value {
        use rusqlite::types::Value;
        self.dicts
            .iter()
            .find(|(c, _)| c == column)
            .and_then(|(_, dict)| match pack_checked(dict, plain) {
                Packed::Value(v) => Some(v),
                Packed::Plain | Packed::Failed => None,
            })
            .map_or_else(|| Value::Text(plain.to_owned()), Value::Blob)
    }

    /// [`Packer::pack`] for an optional value.
    #[must_use]
    pub fn pack_opt(&self, column: &Column, plain: Option<&str>) -> rusqlite::types::Value {
        plain.map_or(rusqlite::types::Value::Null, |p| self.pack(column, p))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn round_trips_with_and_without_a_dictionary() {
        let body = "{\"type\":\"prompt_snapshot\",\"content\":\"".repeat(40) + "\"}";
        let mut plain = Encoder::new(7, None).unwrap();
        let value = plain.encode(&body).unwrap();
        assert_eq!(dict_id(&value), Some(0));
        assert_eq!(decode_blob(&value, None, &mut None).unwrap(), body);

        let samples: Vec<Vec<u8>> = (0..2000)
            .map(|i| {
                format!(
                    "{{\"type\":\"attachment\",\"n\":{i},\"path\":\"/src/m{}.rs\"}}",
                    i % 37
                )
                .into_bytes()
            })
            .collect();
        let dict = train(&samples, 4096).unwrap();
        let mut with = Encoder::new(3, Some(&dict)).unwrap();
        let value = with.encode(&body).unwrap();
        assert_eq!(dict_id(&value), Some(3));
        let mut cache = None;
        assert_eq!(decode_blob(&value, Some(&dict), &mut cache).unwrap(), body);
        assert_eq!(
            decode_blob(&value, None, &mut None),
            Err(DecodeError::MissingDictionary)
        );
    }

    #[test]
    fn short_text_is_not_compressed() {
        assert_eq!(Encoder::new(0, None).unwrap().encode("ok"), None);
    }

    #[test]
    fn corrupt_values_are_errors() {
        let body = "abcdefgh".repeat(100);
        let value = Encoder::new(0, None).unwrap().encode(&body).unwrap();
        // Every truncation and every single-byte flip is an error or the exact
        // original; never other bytes, never a panic.
        for len in 0..value.len() {
            assert!(decode_blob(&value[..len], None, &mut None).is_err());
        }
        for at in 0..value.len() {
            for bit in 0..8 {
                let mut bad = value.clone();
                bad[at] ^= 1 << bit;
                if let Ok(out) = decode_blob(&bad, None, &mut None) {
                    assert_eq!(out, body);
                }
            }
        }
    }
}
