//! CLI install command implementation.

use std::io::IsTerminal;

use color_eyre::eyre::{Result, eyre};
use colored::Colorize;
use dialoguer_multiselect::theme::ColorfulTheme;
use dialoguer_multiselect::{GroupMultiSelect, ItemState};

use harness_locate::{Harness, HarnessKind, InstallationStatus, Scope, Severity, validate_agent_for_harness};

use crate::config::{XenConfig, ProfileManager, ProfileName};
use crate::harness::HarnessConfig;
use crate::install::discovery::{DiscoveryError, discover_skills};
use crate::install::installer::{install_agent, install_command, install_skills};
use crate::install::mcp_installer::{McpInstallOutcome, install_mcp};
use crate::install::{
    AgentInfo, CommandInfo, DiscoveryResult, InstallOptions, InstallTarget, SkillInfo,
};
use harness_locate::McpServer;
use std::collections::HashMap;

type TargetGroup = (
    String,
    Vec<(String, ItemState)>,
    Vec<InstallTarget>,
    Vec<bool>,
    Option<String>, // Harness-level warning (e.g., "HTTP not supported")
);

/// Parsed source input that may include a skill filter
struct ParsedSource {
    /// The GitHub URL (owner/repo format)
    url: String,
    /// Optional skill name extracted from owner/repo/skill-name format
    skill_filter: Option<String>,
}

fn harness_supports_skills(harness_id: &str) -> bool {
    parse_harness_kind(harness_id)
        .and_then(|kind| Harness::locate(kind).ok())
        .and_then(|h| h.skills(&Scope::Global).ok().flatten())
        .is_some()
}

fn harness_supports_agents(harness_id: &str) -> bool {
    parse_harness_kind(harness_id)
        .and_then(|kind| Harness::locate(kind).ok())
        .and_then(|h| h.agents(&Scope::Global).ok().flatten())
        .is_some()
}

fn harness_supports_commands(harness_id: &str) -> bool {
    parse_harness_kind(harness_id)
        .and_then(|kind| Harness::locate(kind).ok())
        .and_then(|h| h.commands(&Scope::Global).ok().flatten())
        .is_some()
}

fn harness_supports_mcp(harness_id: &str) -> bool {
    parse_harness_kind(harness_id)
        .and_then(|kind| Harness::locate(kind).ok())
        .and_then(|h| h.mcp_config_path())
        .is_some()
}

fn count_incompatible_agents(agents: &[AgentInfo], kind: HarnessKind) -> usize {
    agents
        .iter()
        .filter(|a| {
            let issues = validate_agent_for_harness(&a.content, kind);
            issues.iter().any(|i| i.severity == Severity::Error)
        })
        .count()
}

fn count_incompatible_mcps(mcps: &HashMap<String, McpServer>, kind: HarnessKind) -> usize {
    mcps.values()
        .filter(|server| server.validate_capabilities(kind).is_err())
        .count()
}

fn get_incompatible_mcp_names(mcps: &HashMap<String, McpServer>, kind: HarnessKind) -> Vec<String> {
    let mut names: Vec<String> = mcps
        .iter()
        .filter(|(_, server)| server.validate_capabilities(kind).is_err())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

fn is_mcp_compatible(server: &McpServer, kind: HarnessKind) -> bool {
    server.validate_capabilities(kind).is_ok()
}

fn parse_harness_kind(id: &str) -> Option<HarnessKind> {
    match id {
        "claude-code" | "claude" | "cc" => Some(HarnessKind::ClaudeCode),
        "opencode" | "oc" => Some(HarnessKind::OpenCode),
        "goose" => Some(HarnessKind::Goose),
        "amp-code" | "amp" | "ampcode" => Some(HarnessKind::AmpCode),
        "copilot-cli" | "copilot" | "ghcp" => Some(HarnessKind::CopilotCli),
        "crush" => Some(HarnessKind::Crush),
        _ => None,
    }
}

/// Check if a harness has its binary installed on the system.
fn is_harness_binary_installed(kind: HarnessKind) -> bool {
    let harness = Harness::new(kind);
    matches!(
        harness.installation_status(),
        Ok(InstallationStatus::FullyInstalled { .. }) | Ok(InstallationStatus::BinaryOnly { .. })
    )
}

/// Selected components from the discovery result
struct SelectedComponents {
    skills: Vec<SkillInfo>,
    mcp_servers: HashMap<String, McpServer>,
    agents: Vec<AgentInfo>,
    commands: Vec<CommandInfo>,
}

impl SelectedComponents {
    fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.mcp_servers.is_empty()
            && self.agents.is_empty()
            && self.commands.is_empty()
    }
}

pub fn run(
    source: &str,
    force: bool,
    skill_filters: Vec<String>,
    non_interactive: bool,
    target_harness: Option<String>,
    target_profile: Option<String>,
) -> Result<()> {
    let is_tty = std::io::stdin().is_terminal();

    // Parse source to extract skill filter from owner/repo/skill-name format
    let parsed = parse_source(source);
    
    // Combine skill filters from URL path and --skill flags
    let mut all_skill_filters: Vec<String> = skill_filters;
    if let Some(skill_from_url) = parsed.skill_filter {
        all_skill_filters.push(skill_from_url);
    }

    // If we have filters or non-interactive mode, we can proceed without TTY
    let requires_interaction = all_skill_filters.is_empty() && !non_interactive;
    if requires_interaction && !is_tty {
        return Err(eyre!(
            "Interactive mode requires a terminal. Use --yes or specify --skill to run non-interactively."
        ));
    }

    eprintln!("Discovering components from {}...", parsed.url);

    let discovery = discover_skills(&parsed.url).map_err(|e| match e {
        DiscoveryError::InvalidUrl(msg) => eyre!("Invalid URL: {}", msg),
        DiscoveryError::FetchError(e) => eyre!("Failed to fetch repository: {}", e),
        DiscoveryError::NoSkillsFound => eyre!("No installable components found in repository"),
    })?;

    // Build summary of what was found
    let mut found_parts = Vec::new();
    if !discovery.skills.is_empty() {
        found_parts.push(format!("{} skill(s)", discovery.skills.len()));
    }
    if !discovery.mcp_servers.is_empty() {
        found_parts.push(format!("{} MCP server(s)", discovery.mcp_servers.len()));
    }
    if !discovery.agents.is_empty() {
        found_parts.push(format!("{} agent(s)", discovery.agents.len()));
    }
    if !discovery.commands.is_empty() {
        found_parts.push(format!("{} command(s)", discovery.commands.len()));
    }

    if found_parts.is_empty() {
        eprintln!("No installable components found in {}", parsed.url);
        return Ok(());
    }

    eprintln!(
        "Found {} from {}/{}",
        found_parts.join(", "),
        discovery.source.owner,
        discovery.source.repo
    );

    // Determine selected components
    let selected = if !all_skill_filters.is_empty() {
        // Filter mode: select only matching skills
        filter_components(&discovery, &all_skill_filters)?
    } else if non_interactive {
        // Non-interactive: select all components
        select_all_components(&discovery)
    } else {
        // Interactive mode
        select_components(&discovery)?
    };

    if selected.is_empty() {
        if !all_skill_filters.is_empty() {
            let filter_list = all_skill_filters.join(", ");
            eprintln!("No skills found matching: {}", filter_list);
            eprintln!("\nAvailable skills:");
            for skill in &discovery.skills {
                eprintln!("  - {}", skill.name);
            }
        } else {
            eprintln!("No components selected");
        }
        return Ok(());
    }

    // Determine targets
    let targets = if let (Some(harness), Some(profile)) = (&target_harness, &target_profile) {
        // Explicit target specified - validate profile name
        let profile_name = ProfileName::new(profile)
            .map_err(|e| eyre!("Invalid profile name '{}': {}", profile, e))?;
        vec![InstallTarget {
            harness: harness.clone(),
            profile: profile_name,
        }]
    } else if non_interactive || !is_tty {
        // Non-interactive: use active profiles for all harnesses
        get_active_profile_targets(target_harness.as_deref())?
    } else {
        // Interactive mode
        select_targets(&selected)?
    };

    if targets.is_empty() {
        eprintln!("No targets selected");
        return Ok(());
    }

    let options = InstallOptions { force };

    for target in &targets {
        eprintln!("\nInstalling to {}/{}...", target.harness, target.profile);

        // Install skills
        if !selected.skills.is_empty() {
            let report = install_skills(&selected.skills, target, &options);

            for success in &report.installed {
                eprintln!("  + Installed skill: {}", success.skill);
            }
            for skip in &report.skipped {
                eprintln!("  = Skipped skill: {} (already exists)", skip.skill);
            }
            for error in &report.errors {
                eprintln!(
                    "  ! Error installing skill {}: {}",
                    error.skill, error.error
                );
            }
        }

        // Install agents
        if !selected.agents.is_empty() && !harness_supports_agents(&target.harness) {
            eprintln!(
                "  ~ Skipping {} agent(s) - not supported by {}",
                selected.agents.len(),
                target.harness
            );
        } else {
            for agent in &selected.agents {
                match install_agent(agent, target, &options) {
                    Ok(crate::install::installer::InstallOutcome::Installed(success)) => {
                        eprintln!("  + Installed agent: {}", success.skill);
                    }
                    Ok(crate::install::installer::InstallOutcome::Skipped(skip)) => {
                        eprintln!("  = Skipped agent: {} (already exists)", skip.skill);
                    }
                    Err(e) => {
                        eprintln!("  ! Error installing agent {}: {}", agent.name, e);
                    }
                }
            }
        }

        // Install commands
        if !selected.commands.is_empty() && !harness_supports_commands(&target.harness) {
            eprintln!(
                "  ~ Skipping {} command(s) - not supported by {}",
                selected.commands.len(),
                target.harness
            );
        } else {
            for cmd in &selected.commands {
                match install_command(cmd, target, &options) {
                    Ok(crate::install::installer::InstallOutcome::Installed(success)) => {
                        eprintln!("  + Installed command: {}", success.skill);
                    }
                    Ok(crate::install::installer::InstallOutcome::Skipped(skip)) => {
                        eprintln!("  = Skipped command: {} (already exists)", skip.skill);
                    }
                    Err(e) => {
                        eprintln!("  ! Error installing command {}: {}", cmd.name, e);
                    }
                }
            }
        }

        // Install MCP servers
        if !selected.mcp_servers.is_empty() && harness_supports_mcp(&target.harness) {
            let harness_kind = parse_harness_kind(&target.harness);
            for (name, server) in &selected.mcp_servers {
                // Check transport compatibility before attempting installation
                if let Some(kind) = harness_kind
                    && !is_mcp_compatible(server, kind)
                {
                    let transport = match server {
                        McpServer::Stdio(_) => "stdio",
                        McpServer::Sse(_) => "SSE",
                        McpServer::Http(_) => "HTTP",
                    };
                    eprintln!(
                        "  ~ Skipping MCP server: {} ({} transport not supported by {})",
                        name, transport, target.harness
                    );
                    continue;
                }
                match install_mcp(name, server, target, &options) {
                    Ok(McpInstallOutcome::Installed(success)) => {
                        eprintln!("  + Installed MCP server: {}", success.name);
                    }
                    Ok(McpInstallOutcome::Skipped(skip)) => {
                        eprintln!("  = Skipped MCP server: {} ({:?})", skip.name, skip.reason);
                    }
                    Err(e) => {
                        eprintln!("  ! Error installing MCP server {}: {}", name, e);
                    }
                }
            }
        } else if !selected.mcp_servers.is_empty() {
            eprintln!("  ~ Skipping MCP servers (harness does not support MCP)");
        }
    }

    eprintln!("\nDone!");
    Ok(())
}

/// Parse source string, extracting skill filter from owner/repo/skill-name format
fn parse_source(source: &str) -> ParsedSource {
    // Full URL - don't parse for skill name
    if source.starts_with("http://") || source.starts_with("https://") {
        return ParsedSource {
            url: source.to_string(),
            skill_filter: None,
        };
    }

    // Check for owner/repo/skill-name format (skills.sh style)
    let parts: Vec<&str> = source.split('/').collect();
    
    if parts.len() >= 3 {
        // owner/repo/skill-name or owner/repo/path/to/skill
        let owner = parts[0];
        let repo = parts[1];
        let skill_name = parts[2..].join("/");
        
        ParsedSource {
            url: format!("https://github.com/{}/{}", owner, repo),
            skill_filter: Some(skill_name),
        }
    } else if parts.len() == 2 {
        // owner/repo format
        ParsedSource {
            url: format!("https://github.com/{}", source),
            skill_filter: None,
        }
    } else {
        ParsedSource {
            url: source.to_string(),
            skill_filter: None,
        }
    }
}

/// Filter components based on skill name filters
fn filter_components(
    discovery: &DiscoveryResult,
    filters: &[String],
) -> Result<SelectedComponents> {
    let filters_lower: Vec<String> = filters.iter().map(|f| f.to_lowercase()).collect();
    
    let skills: Vec<SkillInfo> = discovery
        .skills
        .iter()
        .filter(|s| {
            let name_lower = s.name.to_lowercase();
            filters_lower.iter().any(|f| {
                // Match by exact name, contains, or path suffix
                name_lower == *f
                    || name_lower.contains(f)
                    || s.path.to_lowercase().contains(f)
            })
        })
        .cloned()
        .collect();

    Ok(SelectedComponents {
        skills,
        mcp_servers: HashMap::new(), // Only skills when filtering
        agents: Vec::new(),
        commands: Vec::new(),
    })
}

/// Select all components (for non-interactive mode)
fn select_all_components(discovery: &DiscoveryResult) -> SelectedComponents {
    SelectedComponents {
        skills: discovery.skills.clone(),
        mcp_servers: discovery.mcp_servers.clone(),
        agents: discovery.agents.clone(),
        commands: discovery.commands.clone(),
    }
}

/// Get active profiles for all (or specified) harnesses
fn get_active_profile_targets(harness_filter: Option<&str>) -> Result<Vec<InstallTarget>> {
    let config = XenConfig::load()?;
    let profiles_dir = XenConfig::profiles_dir()?;
    let manager = ProfileManager::new(profiles_dir);

    let harness_kinds = [
        HarnessKind::OpenCode,
        HarnessKind::ClaudeCode,
        HarnessKind::Goose,
        HarnessKind::AmpCode,
        HarnessKind::CopilotCli,
        HarnessKind::Crush,
    ];

    let mut targets = Vec::new();

    for kind in &harness_kinds {
        // Only consider harnesses with binary installed
        if !is_harness_binary_installed(*kind) {
            continue;
        }
        
        let Ok(harness) = Harness::locate(*kind) else {
            continue;
        };
        let harness_id = harness.id();

        // Filter by harness if specified
        if let Some(filter) = harness_filter {
            if harness_id != filter && !matches_harness_alias(filter, *kind) {
                continue;
            }
        }

        let Ok(profiles) = manager.list_profiles(&harness) else {
            continue;
        };

        if let Some(active) = config.active_profile_for(harness_id) {
            // Check if this profile exists in the list
            let active_name = match ProfileName::new(active) {
                Ok(name) => name,
                Err(_) => continue, // Skip invalid profile names
            };
            if profiles.iter().any(|p| p.as_str() == active_name.as_str()) {
                targets.push(InstallTarget {
                    harness: harness_id.to_string(),
                    profile: active_name,
                });
            }
        }
    }

    if targets.is_empty() {
        return Err(eyre!(
            "No active profiles found. Create and activate a profile first."
        ));
    }

    Ok(targets)
}

fn matches_harness_alias(alias: &str, kind: HarnessKind) -> bool {
    match kind {
        HarnessKind::ClaudeCode => matches!(alias, "claude-code" | "claude" | "cc"),
        HarnessKind::OpenCode => matches!(alias, "opencode" | "oc"),
        HarnessKind::Goose => alias == "goose",
        HarnessKind::AmpCode => matches!(alias, "amp-code" | "amp" | "ampcode"),
        HarnessKind::CopilotCli => matches!(alias, "copilot-cli" | "copilot" | "ghcp"),
        HarnessKind::Crush => alias == "crush",
        _ => false, // Handle any future harness kinds
    }
}

/// Select components to install using grouped multi-select UI
fn select_components(discovery: &DiscoveryResult) -> Result<SelectedComponents> {
    // Build groups for each non-empty category
    let mut groups: Vec<(&str, Vec<String>, Vec<usize>)> = Vec::new();

    if !discovery.skills.is_empty() {
        let names: Vec<String> = discovery.skills.iter().map(|s| s.name.clone()).collect();
        let indices: Vec<usize> = (0..discovery.skills.len()).collect();
        groups.push(("Skills", names, indices));
    }

    if !discovery.mcp_servers.is_empty() {
        let names: Vec<String> = discovery.mcp_servers.keys().cloned().collect();
        let indices: Vec<usize> = (0..discovery.mcp_servers.len()).collect();
        groups.push(("MCP Servers", names, indices));
    }

    if !discovery.agents.is_empty() {
        let names: Vec<String> = discovery.agents.iter().map(|a| a.name.clone()).collect();
        let indices: Vec<usize> = (0..discovery.agents.len()).collect();
        groups.push(("Agents", names, indices));
    }

    if !discovery.commands.is_empty() {
        let names: Vec<String> = discovery.commands.iter().map(|c| c.name.clone()).collect();
        let indices: Vec<usize> = (0..discovery.commands.len()).collect();
        groups.push(("Commands", names, indices));
    }

    if groups.is_empty() {
        return Ok(SelectedComponents {
            skills: Vec::new(),
            mcp_servers: HashMap::new(),
            agents: Vec::new(),
            commands: Vec::new(),
        });
    }

    // All items selected by default
    let defaults: Vec<Vec<bool>> = groups
        .iter()
        .map(|(_, names, _)| vec![true; names.len()])
        .collect();

    let theme = ColorfulTheme::default();
    let mut group_select = GroupMultiSelect::new()
        .with_theme(&theme)
        .with_prompt("Select components to install (Esc to cancel)")
        .defaults(defaults);

    for (category, names, _) in &groups {
        let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        group_select = group_select.group(*category, name_refs);
    }

    let Some(selections) = group_select.interact_opt()? else {
        return Ok(SelectedComponents {
            skills: Vec::new(),
            mcp_servers: HashMap::new(),
            agents: Vec::new(),
            commands: Vec::new(),
        });
    };

    // Map selections back to discovery items
    let mut selected = SelectedComponents {
        skills: Vec::new(),
        mcp_servers: HashMap::new(),
        agents: Vec::new(),
        commands: Vec::new(),
    };

    for (group_idx, selected_indices) in selections.iter().enumerate() {
        let (category, _, _) = &groups[group_idx];
        match *category {
            "Skills" => {
                for &idx in selected_indices {
                    selected.skills.push(discovery.skills[idx].clone());
                }
            }
            "MCP Servers" => {
                let mcp_entries: Vec<_> = discovery.mcp_servers.iter().collect();
                for &idx in selected_indices {
                    let (name, server) = mcp_entries[idx];
                    selected.mcp_servers.insert(name.clone(), server.clone());
                }
            }
            "Agents" => {
                for &idx in selected_indices {
                    selected.agents.push(discovery.agents[idx].clone());
                }
            }
            "Commands" => {
                for &idx in selected_indices {
                    selected.commands.push(discovery.commands[idx].clone());
                }
            }
            _ => {}
        }
    }

    Ok(selected)
}

fn select_targets(selected: &SelectedComponents) -> Result<Vec<InstallTarget>> {
    let config = XenConfig::load()?;
    let profiles_dir = XenConfig::profiles_dir()?;
    let manager = ProfileManager::new(profiles_dir);

    let harness_kinds = [
        HarnessKind::OpenCode,
        HarnessKind::ClaudeCode,
        HarnessKind::Goose,
        HarnessKind::AmpCode,
        HarnessKind::CopilotCli,
        HarnessKind::Crush,
    ];

    let mut groups: Vec<TargetGroup> = Vec::new();

    for kind in &harness_kinds {
        // Only show harnesses with binary installed
        if !is_harness_binary_installed(*kind) {
            continue;
        }
        
        let Ok(harness) = Harness::locate(*kind) else {
            continue;
        };
        let harness_id = harness.id();
        let Ok(profiles) = manager.list_profiles(&harness) else {
            continue;
        };

        if profiles.is_empty() {
            continue;
        }

        let active_profile = config.active_profile_for(harness_id);
        let supports_skills = harness_supports_skills(harness_id);
        let supports_agents = harness_supports_agents(harness_id);
        let supports_commands = harness_supports_commands(harness_id);

        let can_install_skills = supports_skills && !selected.skills.is_empty();
        let can_install_agents = supports_agents && !selected.agents.is_empty();
        let can_install_commands = supports_commands && !selected.commands.is_empty();
        let incompatible_mcp_count = count_incompatible_mcps(&selected.mcp_servers, *kind);
        let compatible_mcp_count = selected.mcp_servers.len() - incompatible_mcp_count;
        let can_install_mcp = compatible_mcp_count > 0;

        // Claude Code MCP support is in development (no global MCP config support)
        let claude_mcp_in_dev =
            *kind == HarnessKind::ClaudeCode && !selected.mcp_servers.is_empty();

        let can_install_anything = can_install_skills
            || can_install_agents
            || can_install_commands
            || (can_install_mcp && !claude_mcp_in_dev);

        let mut skipped: Vec<&str> = Vec::new();
        if !selected.agents.is_empty() && !supports_agents {
            skipped.push("agents");
        }
        if !selected.commands.is_empty() && !supports_commands {
            skipped.push("commands");
        }

        let incompatible_agent_count = if supports_agents && !selected.agents.is_empty() {
            count_incompatible_agents(&selected.agents, *kind)
        } else {
            0
        };

        let mut items_with_states = Vec::new();
        let mut targets = Vec::new();
        let mut defaults = Vec::new();

        for profile in profiles {
            let is_active = active_profile == Some(profile.as_str());
            let label = if is_active {
                format!("{} (active)", profile)
            } else {
                profile.to_string()
            };

            let state = if claude_mcp_in_dev {
                ItemState::Disabled {
                    reason: "MCP: in development".into(),
                }
            } else if !can_install_anything {
                ItemState::Disabled {
                    reason: "no selected components supported".into(),
                }
            } else if !skipped.is_empty()
                || incompatible_agent_count > 0
                || incompatible_mcp_count > 0
            {
                let mut warnings: Vec<String> = Vec::new();
                if !skipped.is_empty() {
                    warnings.push(format!("{} not supported", skipped.join(", ")));
                }
                if incompatible_agent_count > 0 {
                    warnings.push(format!(
                        "{} agent(s) incompatible",
                        incompatible_agent_count
                    ));
                }
                if incompatible_mcp_count > 0 {
                    let names = get_incompatible_mcp_names(&selected.mcp_servers, *kind);
                    warnings.push(format!("{} incompatible", names.join(", ")));
                }
                ItemState::Warning {
                    message: warnings.join("; "),
                }
            } else {
                ItemState::Normal
            };

            let default_selected = is_active && !matches!(state, ItemState::Disabled { .. });

            items_with_states.push((label, state));
            targets.push(InstallTarget {
                harness: harness_id.to_string(),
                profile,
            });
            defaults.push(default_selected);
        }

        let harness_warning = if incompatible_mcp_count > 0 {
            let names = get_incompatible_mcp_names(&selected.mcp_servers, *kind);
            Some(format!("{} incompatible", names.join(", ")))
        } else {
            None
        };
        groups.push((
            harness_id.to_string(),
            items_with_states,
            targets,
            defaults,
            harness_warning,
        ));
    }

    if groups.is_empty() {
        return Err(eyre!(
            "No profiles found. Create a profile first with: xen profile create <harness> <name>"
        ));
    }

    let all_defaults: Vec<Vec<bool>> = groups.iter().map(|(_, _, _, d, _)| d.clone()).collect();

    let theme = ColorfulTheme::default();
    let mut group_select = GroupMultiSelect::new()
        .with_theme(&theme)
        .with_prompt("Select target profiles (Esc to cancel)")
        .defaults(all_defaults);

    for (harness_id, items_with_states, _, _, harness_warning) in &groups {
        let header = if let Some(warning) = harness_warning {
            format!("{} {}", harness_id, format!("⚠ {}", warning).yellow())
        } else {
            harness_id.clone()
        };
        group_select = group_select.group_with_states(&header, items_with_states.clone());
    }

    let Some(selections) = group_select.interact_opt()? else {
        return Ok(Vec::new());
    };

    let mut selected_targets = Vec::new();
    for (group_idx, indices) in selections.iter().enumerate() {
        let (_, _, targets, _, _) = &groups[group_idx];
        for &item_idx in indices {
            selected_targets.push(targets[item_idx].clone());
        }
    }

    Ok(selected_targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_handles_shorthand() {
        let parsed = parse_source("owner/repo");
        assert_eq!(parsed.url, "https://github.com/owner/repo");
        assert!(parsed.skill_filter.is_none());
    }

    #[test]
    fn parse_source_preserves_full_url() {
        let url = "https://github.com/owner/repo";
        let parsed = parse_source(url);
        assert_eq!(parsed.url, url);
        assert!(parsed.skill_filter.is_none());
    }

    #[test]
    fn parse_source_extracts_skill_name() {
        let parsed = parse_source("vercel-labs/agent-skills/vercel-react-best-practices");
        assert_eq!(parsed.url, "https://github.com/vercel-labs/agent-skills");
        assert_eq!(parsed.skill_filter, Some("vercel-react-best-practices".to_string()));
    }

    #[test]
    fn parse_source_handles_nested_skill_path() {
        let parsed = parse_source("owner/repo/skills/my-skill");
        assert_eq!(parsed.url, "https://github.com/owner/repo");
        assert_eq!(parsed.skill_filter, Some("skills/my-skill".to_string()));
    }

    #[test]
    fn matches_harness_alias_works() {
        assert!(matches_harness_alias("claude", HarnessKind::ClaudeCode));
        assert!(matches_harness_alias("cc", HarnessKind::ClaudeCode));
        assert!(matches_harness_alias("opencode", HarnessKind::OpenCode));
        assert!(matches_harness_alias("oc", HarnessKind::OpenCode));
        assert!(!matches_harness_alias("claude", HarnessKind::OpenCode));
    }
}
