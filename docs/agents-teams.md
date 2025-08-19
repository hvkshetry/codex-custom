# Project Agents and Teams (Custom Codex CLI)

This document describes the project-scoped `.codex/` layout, how Codex loads configuration for projects, agents, and teams, and the precedence rules between global, project, agent, and team settings.

## Directory Layout

Place a `.codex/` folder in your project root:

```
<project>/
  .codex/
    config.toml            # Project-wide defaults and MCP servers
    AGENTS.md              # Project system guidance (optional)
    agents/
      <agent-name>/
        config.toml        # Per-agent config (model, provider, flags)
        AGENTS.md          # Per-agent system prompt (preferred)
        mcp.toml           # Optional per-agent MCP servers (map by name)
    teams/
      <team-name>.toml     # Team orchestration config
      TEAM.md              # Optional team-level prompt
```

Codex automatically discovers the nearest `.codex/config.toml` by walking up from the working directory. It deep-merges this project config over the global `~/.codex/config.toml` and then applies CLI `-c key=value` overrides.

## Precedence and Merge Rules

- `CLI -c overrides` > `Agent config` > `Project config` > `Global (~/.codex)`
- `AGENTS.md` prompts:
  - If `.codex/AGENTS.md` exists, Codex prefers it over the global `~/.codex/AGENTS.md`.
  - Per-agent `AGENTS.md` is used for that agent’s system prompt.
  - Teams can specify their own prompt via `teams/<team>.toml` → `prompt_file` or a sibling `TEAM.md`.
- MCP servers:
  - Project MCP servers live under `[mcp_servers]` in `.codex/config.toml`.
  - Per-agent MCP servers default to DISTINCT (no inheritance) and can be defined inline in `agents/<agent>/config.toml` or in `agents/<agent>/mcp.toml`.
  - To opt-in to inheritance, set `inherit_mcp_from_project = true` in the agent’s `config.toml`. When enabled, project servers are merged in; agent keys override on conflict.

## Agent Configuration

`agents/<name>/config.toml` schema (subset):

```
name = "coder"
role = "Implement features and write tests"
model = "gpt-4.1"
model_provider = "openai"
profile = "dev"
prompt_file = "AGENTS.md"  # default if omitted
include_apply_patch_tool = true
include_plan_tool = true
tags = ["code", "rust"]

# default: false (distinct MCP servers)
inherit_mcp_from_project = false

[mcp_servers.build]
command = "./mcp/build-server"
args = []
env = { }
```

Alternatively, place a `mcp.toml` file next to `config.toml` with a top-level map of server definitions. Keys in `mcp.toml` override those defined inline in `config.toml` when names collide.

## Team Configuration

`teams/<name>.toml` schema (subset):

```
name = "dev-team"
mode = "selector"  # round_robin | selector (selector = LLM-based only)
prompt_file = "TEAM.md"     # default if omitted
members = ["researcher", "coder", "reviewer"]

[selector]
model = "gpt-4o-mini"          # REQUIRED when mode = "selector"
prompt_file = "SELECTOR.md"     # optional; otherwise uses a built-in selector prompt
allow_repeated_speaker = false  # instructs selector to avoid repeats, validated at parse time

[termination]
max_turns = 20
mention_text = "TASK_COMPLETE"  # stop when mentioned
```

Notes:
- Teams have their own prompt, distinct from per-agent prompts. This mirrors Agno’s `Team` and AutoGen’s `SelectorGroupChat`, where the orchestrator/manager uses a separate instruction or selector prompt.
- `round_robin`: fixed speaking order based on `members`. Each user turn routes to the next member (unless `@member` override).
- `selector` (LLM-only): the selector model chooses from the listed `members`. There is no heuristic fallback. The selector prompt lists the candidates and enforces “return exactly one name”. If the output is invalid, Codex shows an error and does not advance.

## How Codex Loads Agents and Teams

Codex provides internal loaders (to be wired into CLI flows):

- Discover project `.codex/` directory.
- `list_agents()` scans `.codex/agents/*/config.toml`.
- `load_agent(name)` reads the agent config, loads its `AGENTS.md`, and resolves per-agent MCP servers according to the inheritance flag.
- `list_teams()` enumerates `.codex/teams/*.toml`.
- `load_team(name)` reads the team config and loads the team prompt.

## Selector Prompt

When `mode = "selector"`, Codex constructs a selection prompt for the configured `selector.model`. You can customize it with `selector.prompt_file` (e.g., `SELECTOR.md`). The prompt includes:
- Team name and a summary
- The current user message
- Candidate members (name list)
- Policy (e.g., avoid repeating last speaker)

Output requirement: one exact member name from the candidate list, with no explanations.

## Design Rationale (based on Agno and AutoGen)

- Agno: Teams have distinct system instructions and coordination modes (route/coordinate/collaborate). Teams can pass certain runtime flags to members but member tools remain separate. Success criteria and policies are defined at the team level.
- AutoGen: Group chats include a manager/selector prompt to choose the next speaker (selector_prompt). Termination policies are modular (max messages, text mention) and can be combined. Config can live in code or declarative component files.

We adapt both:
- Team prompt (`TEAM.md` or `prompt_file`) distinct from agent prompts.
- Orchestration modes provided with consistent TOML schema.
- Termination and selector policies are first-class, with defaults and overridable fields.

## Examples

Minimal project `.codex/config.toml`:

```
[mcp_servers.docs]
command = "mcp-docs"
args = []
```

Agent `coder` at `.codex/agents/coder/config.toml`:

```
model = "gpt-4.1"
model_provider = "openai"
inherit_mcp_from_project = false
[mcp_servers.build]
command = "./mcp/build-server"
```

Team `dev-team` at `.codex/teams/dev-team.toml`:

```
mode = "selector"
members = ["researcher", "coder", "reviewer"]
[selector]
model = "gpt-4o-mini"
[termination]
max_turns = 16
```

## FAQs

- Do agents inherit project MCP servers? By default, no. Set `inherit_mcp_from_project = true` in the agent config to merge them (agent keys override).
- Where do team prompts live? In `prompt_file` or `TEAM.md` next to the team config.
- How does Codex prioritize prompts? Agent-level prompts for agents, team-level prompts for orchestrators, project/global AGENTS.md is used as general guidance for single-agent sessions.
