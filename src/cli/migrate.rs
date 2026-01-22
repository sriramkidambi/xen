//! Migration from Bridle to Xen.
//!
//! This module handles migrating existing Bridle configurations to Xen.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::harness::HarnessConfig;

/// Old Bridle configuration directory name.
const BRIDLE_DIR_NAME: &str = "bridle";

/// Old Bridle marker file prefix.
const OLD_MARKER_PREFIX: &str = "BRIDLE_PROFILE_";

/// New Xen marker file prefix.
const NEW_MARKER_PREFIX: &str = "XEN_PROFILE_";

/// Old Bridle manifest filename.
const OLD_MANIFEST_NAME: &str = ".bridle-manifest.json";

/// New Xen manifest filename.
const NEW_MANIFEST_NAME: &str = ".xen-manifest.json";

/// Get the old Bridle config directory path.
fn bridle_config_dir() -> Result<PathBuf> {
    harness_locate::platform::config_dir()
        .map(|d| d.join(BRIDLE_DIR_NAME))
        .map_err(|e| Error::NoConfigFound(e.to_string()))
}

/// Run the migration from Bridle to Xen.
pub fn run() -> Result<()> {
    let bridle_dir = bridle_config_dir()?;
    let xen_dir = crate::config::XenConfig::config_dir()?;

    // Step 1: Check if Bridle config exists
    if !bridle_dir.exists() {
        println!("No Bridle installation found at {}", bridle_dir.display());
        println!("\nNothing to migrate.");
        return Ok(());
    }

    let bridle_config = bridle_dir.join("config.toml");
    if !bridle_config.exists() {
        println!(
            "No Bridle configuration found at {}",
            bridle_config.display()
        );
        println!("\nNothing to migrate.");
        return Ok(());
    }

    println!("Found Bridle installation at {}", bridle_dir.display());

    // Step 2: Check if Xen config already exists
    if xen_dir.exists() {
        print!(
            "\nXen configuration already exists at {}.\nOverwrite? [y/N]: ",
            xen_dir.display()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("Migration cancelled.");
            return Ok(());
        }

        // Remove existing Xen config
        fs::remove_dir_all(&xen_dir)?;
    }

    // Step 3: Copy configuration directory
    println!("\nCopying configuration...");
    copy_dir_recursive(&bridle_dir, &xen_dir)?;
    println!("  Copied {} -> {}", bridle_dir.display(), xen_dir.display());

    // Step 4: Update marker files in all harness directories
    println!("\nUpdating marker files...");
    let markers_updated = update_marker_files()?;
    if markers_updated > 0 {
        println!("  Updated {} marker file(s)", markers_updated);
    } else {
        println!("  No marker files found to update");
    }

    // Step 5: Update manifest filenames
    println!("\nUpdating manifest files...");
    let manifests_updated = update_manifest_files(&xen_dir)?;
    if manifests_updated > 0 {
        println!("  Renamed {} manifest file(s)", manifests_updated);
    } else {
        println!("  No manifest files found to update");
    }

    println!("\n{}", "Migration complete!".to_string());

    // Step 6: Prompt to delete old Bridle config
    print!(
        "\nWould you like to remove the old Bridle configuration? ({}) [y/N]: ",
        bridle_dir.display()
    );
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input == "y" || input == "yes" {
        fs::remove_dir_all(&bridle_dir)?;
        println!("  Removed {}", bridle_dir.display());
    } else {
        println!("  Kept {} (you can remove it manually later)", bridle_dir.display());
    }

    // Step 7: Show uninstall instructions
    println!("\n{}", "─".repeat(60));
    println!("To complete the migration, uninstall Bridle:\n");
    println!("  # If installed via Cargo:");
    println!("  cargo uninstall bridle");
    println!();
    println!("  # If installed via Homebrew:");
    println!("  brew uninstall bridle");
    println!();
    println!("  # If installed via npm:");
    println!("  npm uninstall -g bridle-ai");
    println!();
    println!("  # If installed via pnpm:");
    println!("  pnpm remove -g bridle-ai");
    println!();
    println!("  # If installed via bun:");
    println!("  bun remove -g bridle-ai");
    println!("{}", "─".repeat(60));

    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Update marker files from BRIDLE_PROFILE_* to XEN_PROFILE_* in all harness directories.
fn update_marker_files() -> Result<usize> {
    use harness_locate::{Harness, HarnessKind};

    let mut count = 0;

    for kind in HarnessKind::ALL {
        let harness = Harness::new(*kind);
        let Ok(config_dir) = harness.config_dir() else {
            continue;
        };

        let Ok(entries) = fs::read_dir(&config_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };

            if let Some(profile_name) = name.strip_prefix(OLD_MARKER_PREFIX) {
                let old_path = entry.path();
                let new_name = format!("{}{}", NEW_MARKER_PREFIX, profile_name);
                let new_path = config_dir.join(&new_name);

                // Read content, write to new file, delete old file
                let content = fs::read_to_string(&old_path).unwrap_or_default();
                fs::write(&new_path, content)?;
                fs::remove_file(&old_path)?;
                count += 1;
            }
        }
    }

    Ok(count)
}

/// Update manifest filenames from .bridle-manifest.json to .xen-manifest.json.
fn update_manifest_files(xen_dir: &PathBuf) -> Result<usize> {
    let profiles_dir = xen_dir.join("profiles");
    if !profiles_dir.exists() {
        return Ok(0);
    }

    let mut count = 0;

    // Iterate through harness directories
    for harness_entry in fs::read_dir(&profiles_dir)?.flatten() {
        if !harness_entry.file_type()?.is_dir() {
            continue;
        }

        // Iterate through profile directories
        for profile_entry in fs::read_dir(harness_entry.path())?.flatten() {
            if !profile_entry.file_type()?.is_dir() {
                continue;
            }

            let old_manifest = profile_entry.path().join(OLD_MANIFEST_NAME);
            if old_manifest.exists() {
                let new_manifest = profile_entry.path().join(NEW_MANIFEST_NAME);
                fs::rename(&old_manifest, &new_manifest)?;
                count += 1;
            }
        }
    }

    Ok(count)
}
