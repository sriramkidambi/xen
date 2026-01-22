//! Configuration management for xen.

#![allow(dead_code)]
#![allow(unused_imports)]

mod xen;
pub mod jsonc;
mod manager;
mod profile_name;
mod types;

pub use xen::{XenConfig, TuiConfig, ViewPreference};
pub use manager::ProfileManager;
pub use profile_name::{InvalidProfileName, ProfileName};
pub use types::{McpServerInfo, ProfileInfo, ResourceSummary};
