//! Read-only inventory and doctor for the skill folders coding agents load.
//!
//! [`inventory`] walks the folders in [`locations::LOCATIONS`] under a set of
//! [`Roots`]; [`doctor`] turns that into [`Finding`]s. Nothing in this crate writes
//! to disk or opens a network connection (`tests/no_network_in_skills.rs`,
//! `scripts/egress-check.sh`).

pub mod doctor;
pub mod frontmatter;
pub mod locations;
pub mod lockfiles;
pub mod model;
pub mod scan;

pub use doctor::{doctor, CODEX_BUDGET_CHARS};
pub use locations::Roots;
pub use model::{
    shell_quote, Agent, ContentHash, DriftCopy, Finding, Fix, Install, Mode, SkillName,
    SkillRecord, Source,
};
pub use scan::{inventory, Inventory};
