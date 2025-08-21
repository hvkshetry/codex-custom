About This Fork
===============

This repository is a fork of the OpenAI Codex CLI. It preserves the upstream Apache-2.0 license and NOTICE file.

Key adjustments for public, non-upstream hosting:

- Renamed npm package in `codex-cli/package.json` from `@openai/codex` to `codex-cli` to avoid upstream scope.
- Parameterized native dependency download scripts to support a custom GitHub repository via `--repo owner/repo`.
- Updated CLA workflow to reference the local `docs/CLA.md`.
- Added `SECURITY.md` and `CONTRIBUTING.md`.

What you may want to customize next:

- Replace `REPLACE_ME_OWNER/REPLACE_ME_REPO` in `codex-cli/package.json` with your GitHub slug.
- Decide whether to keep the CLA workflow (`.github/workflows/cla.yml`).
- Adjust release automation to target your repo (see `codex-cli/scripts/stage_release.sh`).
- If you plan to ship your own releases, update any hardcoded URLs in the Rust TUI (e.g., latest release checks) to your repo.

Attribution
-----------

Upstream: https://github.com/openai/codex

