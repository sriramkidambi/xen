use harness_locate::{Harness, HarnessKind};

use crate::config::XenConfig;
use crate::error::{Error, Result};
use crate::harness::HarnessConfig;

pub fn set_config(key: &str, value: &str) -> Result<()> {
    match key {
        "profile_marker" => set_profile_marker(value),
        "editor" => set_editor(value),
        "default_harness" => set_default_harness(value),
        _ => Err(Error::UnknownSetting(key.to_string())),
    }
}

pub fn get_config(key: &str) -> Result<()> {
    let config = XenConfig::load()?;

    match key {
        "profile_marker" => println!("{}", config.profile_marker),
        "editor" => println!("{}", config.editor()),
        "default_harness" => {
            if let Some(harness) = config.default_harness() {
                println!("{}", harness);
            } else {
                println!("(not set)");
            }
        }
        _ => return Err(Error::UnknownSetting(key.to_string())),
    }
    Ok(())
}

fn set_profile_marker(value: &str) -> Result<()> {
    let enabled = match value.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => return Err(Error::InvalidValue(value.to_string())),
    };

    let mut config = XenConfig::load().unwrap_or_default();
    config.set_profile_marker(enabled);
    config.save()?;

    if !enabled {
        cleanup_all_marker_files();
    }

    println!("profile_marker = {}", enabled);
    Ok(())
}

fn set_editor(value: &str) -> Result<()> {
    let mut config = XenConfig::load().unwrap_or_default();

    // Allow clearing with empty string or "none"/"unset"
    let editor = match value.to_lowercase().as_str() {
        "" | "none" | "unset" => None,
        _ => Some(value),
    };

    config.set_editor(editor);
    config.save()?;

    match editor {
        Some(e) => println!("editor = {}", e),
        None => println!("editor = (cleared, will use $EDITOR or vi)"),
    }
    Ok(())
}

fn set_default_harness(value: &str) -> Result<()> {
    let mut config = XenConfig::load().unwrap_or_default();

    // Allow clearing with empty string or "none"/"unset"
    let harness = match value.to_lowercase().as_str() {
        "" | "none" | "unset" => None,
        _ => {
            // Validate harness name
            let valid_harnesses = [
                "claude-code",
                "opencode",
                "goose",
                "amp-code",
                "copilot-cli",
                "crush",
            ];
            if !valid_harnesses.contains(&value) {
                return Err(Error::InvalidValue(format!(
                    "'{}' is not a valid harness. Valid options: {}",
                    value,
                    valid_harnesses.join(", ")
                )));
            }
            Some(value)
        }
    };

    config.set_default_harness(harness);
    config.save()?;

    match harness {
        Some(h) => println!("default_harness = {}", h),
        None => println!("default_harness = (cleared)"),
    }
    Ok(())
}

fn cleanup_all_marker_files() {
    for kind in HarnessKind::ALL {
        let harness = Harness::new(*kind);
        let Ok(config_dir) = harness.config_dir() else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&config_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.starts_with("XEN_PROFILE_") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
