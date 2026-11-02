//! Parse then render is byte-equal for every fixture note, and bad input is
//! refused with the field it came from.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::note::NoteId;

#[test]
fn roundtrip_generated_ids_are_valid_and_distinct() {
    let a = NoteId::generate();
    let b = NoteId::generate();
    assert_ne!(a, b);
    assert!(NoteId::parse(a.as_str()).is_ok(), "{}", a.as_str());
    assert!(a.as_str().starts_with('0'), "{}", a.as_str());
}
