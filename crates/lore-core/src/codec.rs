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
//! The decoder is reached only through SQL: [`register`] installs three scalar
//! functions on every archive connection ([`crate::storage`] and
//! [`crate::storage::migrations`] call it), and the views of migration 0021
//! (`part_body_full`, `message_part_full`, `tool_call_full`) hold the one
//! expression per column that applies them ([`unpack_sql`]):
//!
//! ```sql
//! lore_unpack(body, (SELECT dict FROM zstd_dict WHERE id = lore_dict_miss(body)))
//! ```
//!
//! `lore_unpack` returns plain text unchanged and decompresses a BLOB. Each
//! connection keeps the decoder dictionaries it has prepared, by id, so
//! `lore_dict_miss` returns a value's dictionary id only when that connection
//! has not prepared it yet, and NULL otherwise (and for plain text). The
//! dictionary's bytes are therefore read once per connection, inside the
//! reader's own transaction snapshot, instead of once per value. That is safe
//! because a dictionary is never changed once stored and its id is never
//! reused, and a frame names the zstd id of the dictionary it needs and
//! carries a checksum: a kept dictionary that does not match the frame is
//! fetched again, and a wrong one is an error, never other bytes.
//! `lore_dict_id` reads a value's dictionary id. Every reader selects from
//! those views. A test (`tests/zstd_bodies.rs`) fails when other SQL reads a
//! raw column.
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

/// Largest body a value may decompress to: the largest source file ingest
/// reads ([`crate::ingest::MAX_SOURCE_BYTES`]), which holds every body taken
/// from it.
pub const MAX_DECODED: usize = 64 * 1024 * 1024;

/// Most times larger than its frame a value may decompress to, above
/// [`RATIO_FLOOR`]. zstd can claim far more of a run of one byte; such a value
/// is stored plain instead ([`pack_checked`]).
pub const MAX_RATIO: usize = 1024;

/// Size every value may decompress to whatever its ratio.
pub const RATIO_FLOOR: usize = 1024 * 1024;

/// Most bytes a frame of `frame_len` bytes may decompress to. The decoder
/// allocates the size a frame claims before it decodes it, so this bounds
/// what a corrupt header can make it allocate; every value this build writes
/// is within it.
#[must_use]
pub fn decoded_limit(frame_len: usize) -> usize {
    frame_len
        .saturating_mul(MAX_RATIO)
        .clamp(RATIO_FLOOR, MAX_DECODED)
}

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
        "lore_unpack({expr}, (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_miss({expr})))"
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

/// The zstd id of the dictionary a compressed value's frame needs, `0` for
/// none (or a value too short to say).
fn frame_dict_id(value: &[u8]) -> u32 {
    value
        .get(HEADER_LEN..)
        .and_then(zstd::zstd_safe::get_dict_id_from_frame)
        .map_or(0, std::num::NonZeroU32::get)
}

/// A decoder dictionary prepared once per connection.
struct Prepared {
    /// The `zstd_dict` id.
    id: u32,
    /// The zstd id inside the dictionary, which frames compressed with it name.
    zstd_id: u32,
    dict: zstd::dict::DecoderDictionary<'static>,
}

impl Prepared {
    fn new(id: u32, bytes: &[u8]) -> Self {
        Self {
            id,
            zstd_id: zstd::zstd_safe::get_dict_id_from_dict(bytes)
                .map_or(0, std::num::NonZeroU32::get),
            dict: zstd::dict::DecoderDictionary::copy(bytes),
        }
    }
}

/// Most prepared dictionaries one connection keeps: one per column, with room
/// for a superseded one each.
const DICTS_KEPT: usize = 8;

/// Decoded values one connection remembers.
const MEMO: usize = 4;

/// What the decoding functions keep between rows on one connection: the
/// prepared dictionaries, and the last few values decoded. A view refers to
/// a decoded column more than once (the tool output marker tests and rebuilds
/// `content_json` from `text`), and SQLite evaluates the function at each
/// reference; the memo makes the repeats a compare instead of a decompress.
#[derive(Default)]
struct UnpackState {
    dicts: Vec<Prepared>,
    memo: [Option<(Vec<u8>, String)>; MEMO],
    next: usize,
}

impl UnpackState {
    /// The prepared dictionary `value` needs, if this connection has it.
    fn prepared(&self, value: &[u8]) -> Option<&Prepared> {
        let id = dict_id(value)?;
        let zstd_id = frame_dict_id(value);
        self.dicts
            .iter()
            .find(|p| p.id == id && p.zstd_id == zstd_id)
    }

    /// `lore_dict_miss`: the dictionary id `value` names when decoding it
    /// needs that dictionary's bytes, because this connection has not
    /// prepared it; `None` for a value that names none or one already kept.
    fn miss(&self, value: &[u8]) -> Option<u32> {
        match dict_id(value)? {
            0 => None,
            id => self.prepared(value).is_none().then_some(id),
        }
    }

    /// `lore_unpack` of a compressed value. `dict` is the bytes of the
    /// dictionary it names when [`UnpackState::miss`] asked for them.
    fn unpack(&mut self, value: &[u8], dict: Option<&[u8]>) -> Result<String, DecodeError> {
        let id = dict_id(value).ok_or(DecodeError::Header)?;
        if let (Some(bytes), true) = (dict, id != 0) {
            if self.prepared(value).is_none() {
                if self.dicts.len() >= DICTS_KEPT {
                    self.dicts.clear();
                }
                self.dicts.retain(|p| p.id != id);
                self.dicts.push(Prepared::new(id, bytes));
            }
        }
        if let Some((_, out)) = self.memo.iter().flatten().find(|(v, _)| v == value) {
            return Ok(out.clone());
        }
        let out = if id == 0 {
            decode_frame(value, None)?
        } else {
            let prepared = self.prepared(value).ok_or(DecodeError::MissingDictionary)?;
            decode_frame(value, Some(&prepared.dict))?
        };
        self.memo[self.next] = Some((value.to_vec(), out.clone()));
        self.next = (self.next + 1) % MEMO;
        Ok(out)
    }
}

/// Decompress the compressed value `value` with `dict`, the prepared
/// dictionary its header names (`None` when it names none).
fn decode_frame(
    value: &[u8],
    dict: Option<&zstd::dict::DecoderDictionary<'static>>,
) -> Result<String, DecodeError> {
    let id = dict_id(value).ok_or(DecodeError::Header)?;
    let frame = value.get(HEADER_LEN..).ok_or(DecodeError::Header)?;
    let size = zstd::zstd_safe::get_frame_content_size(frame)
        .map_err(|_| DecodeError::Frame)?
        .ok_or(DecodeError::Size)?;
    let size = usize::try_from(size).map_err(|_| DecodeError::Size)?;
    if size > decoded_limit(frame.len()) {
        return Err(DecodeError::Size);
    }
    let bytes = match (id, dict) {
        (0, _) => zstd::bulk::Decompressor::new()
            .and_then(|mut d| d.decompress(frame, size))
            .map_err(|_| DecodeError::Frame)?,
        (_, None) => return Err(DecodeError::MissingDictionary),
        (_, Some(dict)) => zstd::bulk::Decompressor::with_prepared_dictionary(dict)
            .and_then(|mut d| d.decompress(frame, size))
            .map_err(|_| DecodeError::Frame)?,
    };
    if bytes.len() != size {
        return Err(DecodeError::Frame);
    }
    String::from_utf8(bytes).map_err(|_| DecodeError::NotText)
}

/// Decompress the compressed value `value` with `dict`, the bytes of the
/// dictionary its header names (`None` when it names none or it is missing).
fn decode_blob(value: &[u8], dict: Option<&[u8]>) -> Result<String, DecodeError> {
    let prepared = match (dict_id(value), dict) {
        (Some(0) | None, _) | (_, None) => None,
        (Some(_), Some(bytes)) => Some(zstd::dict::DecoderDictionary::copy(bytes)),
    };
    decode_frame(value, prepared.as_ref())
}

/// Decode a stored value: TEXT as it is, a compressed BLOB decompressed with
/// `dict`, NULL as `None`. For tests and the guard that SQL does the same.
pub fn decode(value: ValueRef<'_>, dict: Option<&[u8]>) -> Result<Option<String>, DecodeError> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(|s| Some(s.to_owned()))
            .map_err(|_| DecodeError::NotText),
        ValueRef::Blob(b) => decode_blob(b, dict).map(Some),
        _ => Err(DecodeError::Type),
    }
}

fn user_error(e: DecodeError) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(e))
}

/// Install `lore_dict_id(value)`, `lore_dict_miss(value)` and
/// `lore_unpack(value, dict)` on `conn`, with fresh per-connection state. All
/// are innocuous, so the schema's views may call them; `lore_dict_miss`
/// depends on what the connection has prepared, so it is not deterministic.
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
    let miss_state = Arc::clone(&state);
    conn.create_scalar_function(
        "lore_dict_miss",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        move |ctx: &Context<'_>| {
            Ok(match ctx.get_raw(0) {
                ValueRef::Blob(b) => miss_state
                    .lock()
                    .map_err(|_| user_error(DecodeError::MissingDictionary))?
                    .miss(b)
                    .map(i64::from),
                _ => None,
            })
        },
    )?;
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
    // A value the decoder would refuse as too large for its frame stays plain.
    if plain.len() > decoded_limit(packed.len().saturating_sub(HEADER_LEN)) {
        return Packed::Plain;
    }
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
        assert_eq!(decode_blob(&value, None).unwrap(), body);

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
        assert_eq!(decode_blob(&value, Some(&dict)).unwrap(), body);
        assert_eq!(
            decode_blob(&value, None),
            Err(DecodeError::MissingDictionary)
        );
    }

    fn dict_from(word: &str) -> Vec<u8> {
        let samples: Vec<Vec<u8>> = (0..2000)
            .map(|i| format!("{{\"{word}\":{i},\"path\":\"/{word}/m{}.rs\"}}", i % 37).into_bytes())
            .collect();
        train(&samples, 4096).unwrap()
    }

    #[test]
    fn a_connection_prepares_each_dictionary_once_and_refetches_one_that_does_not_match() {
        let body = "{\"path\":\"/src/m1.rs\",\"n\":12}".repeat(10);
        let (a, b) = (dict_from("alpha"), dict_from("beta"));
        // The same id for two dictionaries, as if an id were ever reused.
        let value_a = Encoder::new(3, Some(&a)).unwrap().encode(&body).unwrap();
        let value_b = Encoder::new(3, Some(&b)).unwrap().encode(&body).unwrap();
        let mut state = UnpackState::default();
        assert_eq!(state.miss(&value_a), Some(3), "not prepared yet");
        assert_eq!(state.unpack(&value_a, Some(&a)).unwrap(), body);
        assert_eq!(state.miss(&value_a), None, "prepared once");
        assert_eq!(state.unpack(&value_a, None).unwrap(), body);

        // The kept dictionary 3 is not the one value_b's frame names.
        assert_eq!(state.miss(&value_b), Some(3));
        assert_eq!(
            state.unpack(&value_b, None),
            Err(DecodeError::MissingDictionary)
        );
        assert_eq!(state.unpack(&value_b, Some(&b)).unwrap(), body);
        assert_eq!(state.miss(&value_b), None);
        // Plain text and dictionary-less frames never ask for one.
        let plain = Encoder::new(0, None).unwrap().encode(&body).unwrap();
        assert_eq!(state.miss(&plain), None);
        assert_eq!(state.unpack(&plain, None).unwrap(), body);
    }

    #[test]
    fn a_frame_claiming_a_huge_size_fails_before_allocating() {
        // A frame header claiming 1 TiB, then an empty last raw block.
        let mut value = vec![MAGIC, 0, 0, 0, 0, 0x28, 0xB5, 0x2F, 0xFD, 0xE0];
        value.extend_from_slice(&(1_u64 << 40).to_le_bytes());
        value.extend_from_slice(&[1, 0, 0]);
        let started = std::time::Instant::now();
        assert_eq!(decode_blob(&value, None), Err(DecodeError::Size));
        // The size is refused from the header alone: no allocation, no work.
        assert!(started.elapsed() < std::time::Duration::from_millis(50));

        // A real frame over the ratio limit is refused too, and so is never
        // written: the writer stores such a value plain.
        let run = "a".repeat(8 * 1024 * 1024);
        let frame = zstd::bulk::compress(run.as_bytes(), LEVEL).unwrap();
        assert!(run.len() > decoded_limit(frame.len()));
        let mut value = vec![MAGIC, 0, 0, 0, 0];
        value.extend_from_slice(&frame);
        assert_eq!(decode_blob(&value, None), Err(DecodeError::Size));
        let dict = Dict::new(1, dict_from("gamma"));
        assert_eq!(pack_checked(&dict, &run), Packed::Plain);
        let text = "{\"path\":\"/gamma/m3.rs\"}".repeat(4000);
        let Packed::Value(v) = pack_checked(&dict, &text) else {
            panic!("compressible text within the limit is compressed");
        };
        assert_eq!(decode_blob(&v, Some(&dict.bytes)).unwrap(), text);
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
            assert!(decode_blob(&value[..len], None).is_err());
        }
        for at in 0..value.len() {
            for bit in 0..8 {
                let mut bad = value.clone();
                bad[at] ^= 1 << bit;
                if let Ok(out) = decode_blob(&bad, None) {
                    assert_eq!(out, body);
                }
            }
        }
    }
}
