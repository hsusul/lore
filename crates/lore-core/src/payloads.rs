//! Inline binary payloads in part JSON: screenshots, image attachments, PDFs.
//!
//! Agent logs embed them as base64, often megabytes each, and the same image
//! recurs across turns. Kept inline they were the largest share of an
//! archive, and every byte of them went through the secret scanner, which can
//! find nothing in base64 but noise. Before a session is written, each string
//! of at least [`MIN_PAYLOAD_CHARS`] that is a `data:` URL or plain base64 is
//! moved into the blob store, stored once by content hash, and replaced by
//! `lore-blob:<hash>`. The blob row is `opaque_excluded`: never scanned,
//! never indexed. The bytes kept are the base64 text exactly as logged.

use serde_json::Value;

use crate::model::ParsedSession;
use crate::storage::blob::{BlobStore, StagedBlob};
use crate::storage::Result;

/// Shorter strings stay inline: they cost little and may be ordinary text.
pub const MIN_PAYLOAD_CHARS: usize = 4096;
/// What replaces a moved payload, followed by its content hash.
pub const REFERENCE_PREFIX: &str = "lore-blob:";

/// A payload moved into the blob store, the media type its JSON named, and
/// the part it came from, by message `seq` and part `ordinal`.
#[derive(Debug, Clone)]
pub struct Moved {
    pub staged: StagedBlob,
    pub media_type: String,
    pub part: (i64, i64),
}

/// Move every inline payload in `parsed`'s part JSON into `blobs`, rewriting
/// the JSON in place. Returns the moved payloads, one per part and content.
pub fn externalize(parsed: &mut ParsedSession, blobs: &BlobStore) -> Result<Vec<Moved>> {
    let mut moved: Vec<Moved> = Vec::new();
    for message in &mut parsed.messages {
        let seq = message.seq;
        for part in &mut message.parts {
            externalize_part(part, (seq, part.ordinal), blobs, &mut moved)?;
        }
    }
    Ok(moved)
}

fn externalize_part(
    part: &mut crate::model::ParsedPart,
    at: (i64, i64),
    blobs: &BlobStore,
    moved: &mut Vec<Moved>,
) -> Result<()> {
    let Some(json) = part
        .content_json
        .as_deref()
        .filter(|j| j.len() >= MIN_PAYLOAD_CHARS)
    else {
        return Ok(());
    };
    let Ok(mut value) = serde_json::from_str::<Value>(json) else {
        return Ok(());
    };
    let before = moved.len();
    walk(&mut value, None, at, blobs, moved)?;
    if moved.len() > before {
        part.content_json = serde_json::to_string(&value).ok();
    }
    Ok(())
}

fn walk(
    value: &mut Value,
    media_type: Option<&str>,
    at: (i64, i64),
    blobs: &BlobStore,
    moved: &mut Vec<Moved>,
) -> Result<()> {
    match value {
        Value::String(text) if is_payload(text) => {
            let media = data_url_media_type(text)
                .or(media_type)
                .unwrap_or("application/octet-stream")
                .to_owned();
            let staged = blobs.stage(text.as_bytes())?;
            *text = format!("{REFERENCE_PREFIX}{}", staged.content_hash());
            // The same image twice in one part is one link.
            let hash = staged.content_hash();
            if !moved
                .iter()
                .any(|m| m.part == at && m.staged.content_hash() == hash)
            {
                moved.push(Moved {
                    staged,
                    media_type: media,
                    part: at,
                });
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, media_type, at, blobs, moved)?;
            }
        }
        Value::Object(map) => {
            let own = ["media_type", "mime_type", "mimeType"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str))
                .map(str::to_owned);
            let media = own.as_deref().or(media_type);
            for (_, item) in map.iter_mut() {
                walk(item, media, at, blobs, moved)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A `data:` URL, or a long run of base64 and nothing else.
fn is_payload(text: &str) -> bool {
    if text.len() < MIN_PAYLOAD_CHARS {
        return false;
    }
    if text.starts_with("data:") {
        return text.get(..128).unwrap_or(text).contains(";base64,");
    }
    text.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_' | b'\n' | b'\r')
    })
}

/// The media type a `data:` URL names, like `image/png`.
fn data_url_media_type(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("data:")?;
    let end = rest.find([';', ','])?;
    Some(&rest[..end]).filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_long_base64_or_a_data_url() {
        let long = "QUJD".repeat(1100);
        assert!(is_payload(&long));
        assert!(is_payload(&format!("data:image/png;base64,{long}")));
        assert!(!is_payload("QUJD"));
        assert!(!is_payload(&format!("{long} and some prose")));
    }

    #[test]
    fn data_url_media_type_parsing() {
        assert_eq!(
            data_url_media_type("data:image/jpeg;base64,xx"),
            Some("image/jpeg")
        );
        assert_eq!(
            data_url_media_type("data:image/svg+xml;base64,abc"),
            Some("image/svg+xml")
        );
        assert_eq!(
            data_url_media_type("data:application/pdf;base64,abc"),
            Some("application/pdf")
        );
        assert_eq!(
            data_url_media_type("data:text/plain;charset=utf-8;base64,abc"),
            Some("text/plain")
        );
        assert_eq!(data_url_media_type("data:;base64,xx"), None);
        assert_eq!(data_url_media_type("data:,abc"), None);
        assert_eq!(data_url_media_type("not_a_data_url"), None);
    }

    #[test]
    fn is_payload_boundaries_and_charsets() {
        let exact_min = "A".repeat(MIN_PAYLOAD_CHARS);
        assert!(is_payload(&exact_min));

        let under_min = "A".repeat(MIN_PAYLOAD_CHARS - 1);
        assert!(!is_payload(&under_min));

        // URL-safe base64 and standard base64 symbols
        let url_safe = "A-B_C=D/E+F\r\n".repeat(400);
        assert!(is_payload(&url_safe));

        // Non-base64 character rejects plain text
        let invalid_char = format!("{}!", "A".repeat(MIN_PAYLOAD_CHARS));
        assert!(!is_payload(&invalid_char));

        // data URL requiring ;base64,
        let not_b64_data = format!("data:image/png;utf8,{}", "A".repeat(MIN_PAYLOAD_CHARS));
        assert!(!is_payload(&not_b64_data));
    }

    #[test]
    fn externalize_moves_large_payload_and_updates_content_json() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let long = "QUJD".repeat(1100);
        let mut session = ParsedSession::new("s1");
        session.messages.push(crate::model::ParsedMessage {
            seq: 1,
            segment_ix: 0,
            native_uuid: None,
            parent_native_uuid: None,
            role: crate::model::Role::User,
            event_kind: crate::model::EventKind::Message,
            is_sidechain: false,
            ts: Some(1000),
            model: None,
            tokens: crate::model::Tokens::default(),
            stop_reason: None,
            source_offset: None,
            metadata_json: None,
            parts: vec![
                crate::model::ParsedPart {
                    ordinal: 0,
                    kind: crate::model::PartKind::Text,
                    text: None,
                    content_json: Some(format!("{{\"image\":\"data:image/png;base64,{long}\"}}")),
                    searchable: true,
                    metadata_json: None,
                },
                crate::model::ParsedPart {
                    ordinal: 1,
                    kind: crate::model::PartKind::Text,
                    text: None,
                    content_json: Some("{\"short\":\"text\"}".into()),
                    searchable: true,
                    metadata_json: None,
                },
            ],
        });

        let moved = externalize(&mut session, &blobs).unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].media_type, "image/png");
        assert_eq!(moved[0].part, (1, 0));
        assert!(session.messages[0].parts[0]
            .content_json
            .as_ref()
            .unwrap()
            .contains(REFERENCE_PREFIX));
        assert_eq!(
            session.messages[0].parts[1].content_json.as_deref(),
            Some("{\"short\":\"text\"}")
        );
    }
}
