//! Read-only inventory and doctor for the skill folders coding agents load.

pub mod frontmatter;
pub mod locations;
pub mod lockfiles;
pub mod model;
pub mod scan;

pub use model::{
    Agent, ContentHash, DriftCopy, Finding, Install, Mode, SkillName, SkillRecord, Source,
};
pub use scan::{inventory, Inventory};
