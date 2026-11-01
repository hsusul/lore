//! Read-only inventory and doctor for the skill folders coding agents load.

pub mod locations;
pub mod model;

pub use model::{
    Agent, ContentHash, DriftCopy, Finding, Install, Mode, SkillName, SkillRecord, Source,
};
