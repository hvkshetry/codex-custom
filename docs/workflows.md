Project Workflows (Preview)

Overview
- Workflows let you define a sequential flow across agents and teams using TOML files under `.codex/workflows/`.
- Each step runs as a clean session (agent or team) with its own prompt and optional `max_turns`.
- Team steps use the configured team mode (round_robin/selector). `@member` overrides still work inside the team step.

Directory
- `.codex/workflows/<name>.toml`

Minimal Schema (sequential)
```
name = "release-pipeline"
description = "Plan → Implement → Review"
steps = ["plan", "implement", "review"]

[step.plan]
type = "team"        # or "agent"
id = "dev-team"
prompt = "Plan the scope and risks"
max_turns = 4

[step.implement]
type = "agent"
id = "coder"
prompt = "Implement features based on the plan"
max_turns = 6

[step.review]
type = "agent"
id = "reviewer"
prompt = "Review and list issues"
max_turns = 4
```

Usage (planned)
- CLI: `codex-custom workflow run <name>`
- TUI: `/workflow run <name>`
- The runner executes steps in order, shows progress, and writes a run log to `.codex/log/workflows/<run-id>.jsonl`.

Notes
- Each step creates a new clean session; there is no mid-session hot-swapping.
- Outputs from previous steps can be summarized and injected into subsequent prompts (planned).
- Graph/DAG flows with conditional edges and parallel branches may be added later; the initial release focuses on sequential flows.
