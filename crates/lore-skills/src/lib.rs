//! Read-only inventory and doctor for the skill folders coding agents load.
//!
//! Nothing in this crate writes to disk or opens a network connection
//! (`tests/no_network_in_skills.rs`, `scripts/egress-check.sh`).

pub mod frontmatter;
pub mod locations;
pub mod model;

pub use locations::Roots;
pub use model::Agent;
