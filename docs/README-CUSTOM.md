# Codex Custom CLI (codex-custom)

This repository includes an extended build of the OpenAI Codex CLI that adds first‑class support for project‑level configuration and agentic workflows beyond coding. The custom binary is named `codex-custom` to clearly distinguish it from the official `codex` CLI.

## Motivation

Codex CLI is fantastic for coding, but many teams want to drive broader, repeatable workflows that coordinate tools and services not limited to source code. This custom build aims to:

- Treat projects as first‑class: discover `.codex/` in your repo and merge its config over your global settings.
- Let projects define their own Model Context Protocol (MCP) servers so tools are available where work happens.
- Add optional agentic primitives (agents, teams, workflows) that can be layered onto projects when needed.
- Keep compatibility: if you just want a single‑agent chat, you can ignore agents/teams and still benefit from project‑level MCP servers.

## Key Customizations

- New binary: `codex-custom` (the official CLI remains `codex`).
- Project config merge: the nearest `.codex/config.toml` is discovered from CWD and deep‑merged over `~/.codex/config.toml` before CLI `-c` overrides. Project values win over global by default.
- Project MCP servers: define `[mcp_servers]` in `.codex/config.toml` so project tools are available on startup.
- Optional agents/teams/workflows (TUI only):
  - `/agents`, `/teams`, `/workflows` to discover project content in `.codex/agents`, `.codex/teams`, `.codex/workflows`.
  - Agents support opt‑in inheritance of project MCP servers: set `inherit_mcp_from_project = true` per agent to merge.
  - `/init` scaffolds a project `.codex/` with sample files (agents/teams/workflows) when you want to use them.
- Status and trust: project trust can be set to skip approvals friction; trust is stored under `[projects]` in `~/.codex/config.toml`.

## How It Works (Config Precedence)

Precedence for configuration values (lowest → highest):

1. Global `~/.codex/config.toml`
2. Project `.codex/config.toml` (nearest to CWD; deep‑merged over global)
3. CLI `-c key=value` overrides

Within an agent session (optional):

- Agent config can override model, provider, and local flags.
- MCP servers for the active session default to the agent’s set. If `inherit_mcp_from_project = true` is set in the agent, project MCP servers are merged; agent keys override on conflict.

## Differences from Upstream

- Adds project config discovery and merge over global config.
- Adds project MCP servers at startup without requiring agents/teams.
- Includes new TUI slash commands: `/agents`, `/teams`, `/workflows`, extended `/init`.
- Ships a separate binary name (`codex-custom`) to avoid confusion with the official CLI.

## Example Project Configuration

`.codex/config.toml` at the project root:

```toml
# Project-scoped MCP servers
[mcp_servers.openbb]
command = "/abs/path/to/venv/bin/python"
args = ["/abs/path/to/openbb-mcp-server.py"]
env = { PYTHONPATH = "/abs/project/src" }

[mcp_servers.github]
command = "docker"
args = ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]

[mcp_servers.deepwiki]
command = "uvx"
args = ["mcp-proxy", "https://mcp.deepwiki.com/sse"]
```

Notes:

- Ensure `command` paths exist and are executable in your environment; otherwise the TUI will report that a client failed to start.
- Keep server names to `^[a-zA-Z0-9_-]+$`.

## Workflow Example (Optional)

`.codex/workflows/sample.toml`:

```toml
name = "sample"
description = "Sample sequential workflow"
steps = ["plan", "implement"]

[step.plan]
type = "team"
id = "dev-team"
prompt = "Draft a short plan."
max_turns = 1

[step.implement]
type = "agent"
id = "dev"
prompt = "Implement the plan with concise steps."
max_turns = 1
```

## Safety and Approvals

- `approval_policy` and `sandbox_mode` can be set globally, in a profile, or via CLI flags.
- Trusting a project (via onboarding or `set_project_trusted`) stores trust in `~/.codex/config.toml` under `[projects]` and reduces friction for common operations.

## Setup

See `docs/SETUP.md` for build, install, and verification steps.

## License

The customizations in this tree are provided under the same license as the upstream Codex CLI unless otherwise noted. See `docs/LICENSE-CUSTOM.md` for the attribution notice and terms. The upstream project license remains in `LICENSE`.
