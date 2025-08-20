# Setup Guide – codex-custom

This guide explains how to build, install, and configure the custom Codex CLI (`codex-custom`). The official CLI remains available as `codex`.

## 1) Prerequisites

- Rust toolchain (rustup) and Cargo
- A shell where `~/.local/bin` is on your PATH (or equivalent)
- Optional: Docker (for some MCP servers), Python/virtualenvs for Python-based MCP servers

## 2) Build the custom binary

Debug build (fastest to iterate):

```bash
cd codex-src/codex-rs
cargo build -p codex-cli --bin codex-custom
```

Release build (smaller/faster binary):

```bash
cd codex-src/codex-rs
cargo build -p codex-cli --bin codex-custom --release
```

## 3) Install to PATH (symlink)

Choose the artifact you just built:

- Debug: `codex-src/codex-rs/target/debug/codex-custom`
- Release: `codex-src/codex-rs/target/release/codex-custom`

Create or update the symlink:

```bash
mkdir -p ~/.local/bin
ln -sf "$(pwd)/target/debug/codex-custom" ~/.local/bin/codex-custom  # or target/release
```

Verify path resolution:

```bash
which codex-custom
codex-custom --help | head -20
```

You should see help text that includes the enhanced subcommands and TUI slash commands
(e.g. `/agents`, `/teams`, `/workflows` in the TUI tips).

## 4) Configure global settings (optional)

Global config lives at `~/.codex/config.toml`. You can set defaults for model, provider, history, etc., and define global MCP servers if desired. Project config will deep‑merge over these values.

## 5) Configure a project `.codex/`

In your project root, add `.codex/config.toml` with project‑level MCP servers:

```toml
[mcp_servers.github]
command = "docker"
args = ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]

[mcp_servers.deepwiki]
command = "uvx"
args = ["mcp-proxy", "https://mcp.deepwiki.com/sse"]
```

Tips:

- Use absolute paths for Python venvs and scripts.
- Keep server names to `^[a-zA-Z0-9_-]+$`.
- If a server fails to start, check `~/.codex/log/codex-tui.log` for “MCP client … failed to start”.

## 6) Run codex-custom in your project

From the project directory:

```bash
codex-custom
```

On startup, the CLI discovers the nearest `.codex/config.toml`, merges it over global config, and initializes MCP servers. Use `/status` for session info; errors about MCP startup will be surfaced in the UI and logs.

## 7) Optional: Agents, Teams, Workflows

- Run `/init` to scaffold `.codex/` with sample agents/teams/workflows if you want those features.
- Agents inherit project MCP servers only if you set `inherit_mcp_from_project = true` in `.codex/agents/<agent>/config.toml`.

## 8) Safety and trust

- Approval and sandbox settings can be set via config or flags.
- When a project is trusted (set during onboarding or via API), the trust state is recorded under `[projects]` in `~/.codex/config.toml`.

## 9) Troubleshooting

- “No tools listed” or missing project tools:
  - Verify you are running `codex-custom` (not `codex`).
  - Check `~/.codex/log/codex-tui.log` for MCP startup errors.
  - Confirm absolute paths and permissions for MCP `command` and `args`.
- Agents don’t see project tools:
  - Set `inherit_mcp_from_project = true` in the agent’s config, or copy the MCP definitions into the agent.

## 10) Keeping both CLIs

- `codex-custom` → custom build with project MCP and agentic extensions.
- `codex` → official upstream CLI.

