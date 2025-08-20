use crate::config::{find_project_codex_dir, resolve_preliminary_cwd, ConfigToml};
use crate::config_types::McpServerConfig;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Project-level discovery entrypoint. Returns the canonical `.codex` dir if present.
pub fn discover_project_codex_dir(cwd_override: Option<PathBuf>) -> std::io::Result<Option<PathBuf>> {
    let cwd = resolve_preliminary_cwd(cwd_override)?;
    Ok(find_project_codex_dir(&cwd))
}

/// Minimal agent configuration schema (per-agent `config.toml`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentConfigToml {
    pub name: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub profile: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub include_apply_patch_tool: Option<bool>,
    pub include_plan_tool: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// When true, merge project `mcp_servers` into this agent (agent overrides win).
    /// Default: false (agent MCP servers are distinct).
    #[serde(default)]
    pub inherit_mcp_from_project: bool,
    /// Inline MCP servers for this agent (alternative to `mcp.toml`).
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub dir: PathBuf,
    pub config: AgentConfigToml,
    pub prompt: Option<String>,
    /// Effective MCP servers for this agent after merge policy.
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

/// Team configuration schema stored at `.codex/teams/<name>.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TeamConfigToml {
    pub name: Option<String>,
    /// One of: route | coordinate | collaborate | round_robin | selector
    pub mode: Option<String>,
    /// Path to a team prompt (defaults to `TEAM.md` adjacent to this file).
    pub prompt_file: Option<PathBuf>,
    /// Member agent names.
    #[serde(default)]
    pub members: Vec<String>,
    /// Optional termination configuration.
    #[serde(default)]
    pub termination: HashMap<String, toml::Value>,
    /// Optional selector configuration when `mode = "selector"`.
    #[serde(default)]
    pub selector: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone)]
pub struct TeamDefinition {
    pub file: PathBuf,
    pub config: TeamConfigToml,
    pub prompt: Option<String>,
}

pub fn list_agents(project_codex_dir: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    let agents_dir = project_codex_dir.join("agents");
    if !agents_dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&agents_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("config.toml").exists() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn load_agent(
    project_codex_dir: &Path,
    name: &str,
    project_cfg: &ConfigToml,
) -> std::io::Result<AgentDefinition> {
    let dir = project_codex_dir.join("agents").join(name);
    let cfg_path = dir.join("config.toml");
    let raw = fs::read_to_string(&cfg_path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to read agent config {}: {e}", cfg_path.display()),
        )
    })?;
    let mut cfg: AgentConfigToml = toml::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse agent config {}: {e}", cfg_path.display()),
        )
    })?;

    // Load prompt: default to AGENTS.md inside agent dir.
    let prompt_path = cfg
        .prompt_file
        .clone()
        .map(|p| if p.is_relative() { dir.join(p) } else { p })
        .unwrap_or_else(|| dir.join("AGENTS.md"));
    let prompt = fs::read_to_string(&prompt_path).ok().and_then(|s| {
        let s = s.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    });

    // Resolve MCP servers. Default policy: distinct (do not inherit).
    let mut mcp = cfg.mcp_servers.clone();
    // Load optional `mcp.toml` file if present.
    let mcp_file = dir.join("mcp.toml");
    if mcp_file.exists() {
        if let Ok(s) = fs::read_to_string(&mcp_file) {
            if let Ok(val) = toml::from_str::<HashMap<String, McpServerConfig>>(&s) {
                // keys in mcp.toml override any inline entries with the same name.
                for (k, v) in val.into_iter() {
                    mcp.insert(k, v);
                }
            }
        }
    }
    // Optionally inherit from project config.
    if cfg.inherit_mcp_from_project {
        if let Some(project_map) = Some(project_cfg.mcp_servers.clone()) {
            for (k, v) in project_map.into_iter() {
                mcp.entry(k).or_insert(v);
            }
        }
    }

    // Normalize name default.
    if cfg.name.is_none() {
        cfg.name = Some(name.to_string());
    }

    Ok(AgentDefinition {
        dir,
        config: cfg,
        prompt,
        mcp_servers: mcp,
    })
}

pub fn list_teams(project_codex_dir: &Path) -> std::io::Result<Vec<String>> {
    let teams_dir = project_codex_dir.join("teams");
    if !teams_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&teams_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn load_team(project_codex_dir: &Path, name: &str) -> std::io::Result<TeamDefinition> {
    let teams_dir = project_codex_dir.join("teams");
    let file = teams_dir.join(format!("{name}.toml"));
    let raw = fs::read_to_string(&file).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to read team config {}: {e}", file.display()),
        )
    })?;
    let cfg: TeamConfigToml = toml::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse team config {}: {e}", file.display()),
        )
    })?;

    // Load team prompt (TEAM.md next to file by default).
    let prompt_path = {
        let default_path = file.with_file_name("TEAM.md");
        match cfg.prompt_file.clone() {
            None => default_path,
            Some(p) if !p.is_relative() => p,
            Some(p) => {
                let parent = file.parent().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "team config path has no parent directory: {}",
                            file.display()
                        ),
                    )
                })?;
                parent.join(p)
            }
        }
    };
    let prompt = fs::read_to_string(&prompt_path).ok().and_then(|s| {
        let s = s.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    });

    Ok(TeamDefinition { file, config: cfg, prompt })
}
