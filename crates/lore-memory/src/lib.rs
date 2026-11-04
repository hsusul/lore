//! Lore memory: notes a person can read, edit, grep, and diff without Lore.
//!
//! A note is a markdown file with YAML frontmatter, kept in a git repository
//! called the vault ([`vault`]). The vault is the source of truth. The full-text
//! index ([`index`]) lives in `<vault>/.lore/index.db`, is gitignored, and is
//! rebuilt from the vault alone, so it can never hold anything the vault lacks.
//! Agents only propose ([`inbox`]); a person accepts ([`review`]).
//!
//! Recall ([`recall`]) returns the notes for a directory's repository, each with
//! a [`anchor::Freshness`] from checking its anchors against the working tree.
//!
//! Eval ([`eval`]) scores a policy for accepting proposals without a person
//! against the decisions a person already made, offline. [`policy`] holds the
//! policies it scores.
//!
//! The miner ([`mine`]) reads the archive for user turns that correct the
//! agent, the raw material of a lesson; [`filing`] files a lesson into the
//! inbox.
//!
//! This crate has no network capability (`tests/no_network_in_memory.rs`,
//! `scripts/egress-check.sh`). It spawns the `git` binary and nothing else.

pub mod anchor;
pub mod eval;
pub mod filing;
pub mod inbox;
pub mod index;
pub mod mine;
pub mod note;
pub mod policy;
pub mod recall;
pub mod repo;
pub mod review;
pub mod session_start;
pub mod vault;
