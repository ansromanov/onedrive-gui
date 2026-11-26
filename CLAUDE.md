# CLAUDE.md

The canonical instructions are in **[AGENTS.md](AGENTS.md)**; read it first. This file only
adds what is specific to Claude Code.

- Work in a dedicated git worktree, never in the main checkout.
- When you start subagents with the Agent tool, pass `isolation: "worktree"`.
- Do not report a change as done until the gate in AGENTS.md passes and the pull request is merged or you have said exactly what blocks it.
