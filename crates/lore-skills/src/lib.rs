//! Read-only inventory and doctor for the skill folders coding agents load.
//!
//! [`inventory`] walks the folders in [`locations::LOCATIONS`] under a set of
//! [`Roots`]. Nothing in this crate writes to disk or opens a network
//! connection (`tests/no_network_in_skills.rs`, `scripts/egress-check.sh`).

pub mod frontmatter;
pub mod locations;
pub mod lockfiles;
pub mod model;
pub mod scan;

pub use locations::Roots;
pub use model::{Agent, ContentHash, Install, Mode, SkillName, SkillRecord, Source};
pub use scan::{inventory, Inventory};
