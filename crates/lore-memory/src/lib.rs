//! Lore memory: notes a person can read, edit, grep, and diff without Lore.
//!
//! A note is a markdown file with YAML frontmatter, kept in a git repository
//! called the vault ([`vault`]). The vault is the source of truth. The full-text
//! index ([`index`]) lives in `<vault>/.lore/index.db`, is gitignored, and is
//! rebuilt from the vault alone, so it can never hold anything the vault lacks.
//! Agents only propose ([`inbox`]); a person accepts ([`review`]).
//!
//! This crate has no network capability (`tests/no_network_in_memory.rs`,
//! `scripts/egress-check.sh`). It spawns the `git` binary and nothing else.

pub mod inbox;
pub mod index;
pub mod note;

pub mod vault;
