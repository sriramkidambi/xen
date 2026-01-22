//! Init command implementation.

use harness_locate::{Harness, HarnessKind};

use crate::config::{XenConfig, ProfileManager};
use crate::error::Result;

pub fn run_init() -> Result<()> {
    let config_dir = XenConfig::config_dir()?;
    let config_path = XenConfig::config_path()?;

    if config_path.exists() {
        println!("Already initialized at {}", config_dir.display());
        return Ok(());
    }

    std::fs::create_dir_all(&config_dir)?;

    let profiles_dir = config_dir.join("profiles");
    std::fs::create_dir_all(&profiles_dir)?;

    let config = XenConfig::default();
    config.save()?;

    let manager = ProfileManager::new(profiles_dir);
    for kind in HarnessKind::ALL {
        let harness = Harness::new(*kind);
        let _ = manager.create_from_current_if_missing(&harness);
    }

    println!("Initialized xen at {}", config_dir.display());
    Ok(())
}
