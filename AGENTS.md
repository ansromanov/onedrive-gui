# AGENTS.md — onedrive-gui

> Canonical instructions for this repository, read by Claude Code, Codex and other
> agents. Keep it short: no file tree and no symbol index, they rot. Use `ls`, `grep`
> and the module docs instead.

onedrive-gui is a Windows-style tray app, settings and Dolphin sync marks for abraunegg/onedrive. Licence: MIT OR Apache-2.0. This repository is public: nothing private
(credentials, host names, internal paths, other people's licensed material) goes in.

## Layout

- `daemon/` (Rust) — runs one `onedrive --monitor --confdir <account>` per account, parses its output, serves status on D-Bus (`org.onedrivegui.Daemon`).
- `app/` (C++/Qt6/QML) — tray icon, flyout, settings, notifications; talks only to the daemon.
- `dolphin/` — optional KDE plugin with sync marks. `packaging/arch/` — the pacman package.

## Rules worth knowing

- The GUI never touches synced files or the onedrive database.
- The daemon runs onedrive only as `--monitor --confdir`, `--confdir --display-quota`, `--resync --resync-auth` after a settings change, and `--reauth --auth-files`. It stops onedrive with SIGTERM only, never SIGKILL.
- The unit conflicts with `onedrive.service` so two instances never sync one account.

## Gate

Run this before opening a pull request. CI runs the same checks.

```sh
just check   # rustfmt, clippy -D warnings, shellcheck, the file-size gate, then tests/run.sh
```

## Testing

`tests/run.sh` runs the daemon's unit tests, a smoke test on a private D-Bus with a fake onedrive, and the Dolphin plugin check. No real account is ever involved. Add or update tests with every behaviour change; say in the pull request why
something cannot be tested.

## Workflow

- Work in a git worktree on a branch, never on `main`. Fetch and base the branch on the latest `origin/main`.
- `main` is protected: every change is a pull request with green CI. Never push to `main`.
- Commit messages and pull request titles are [Conventional Commits](https://www.conventionalcommits.org)
  (`feat:`, `fix:`, `docs:`, `refactor:`, `ci:`, `chore:`); the changelog and versions come from them.
- Files stay at or below 800 production lines (`scripts/check-file-size.py`). Split the module; never raise
  the limit or add to `scripts/file-size-baseline.txt` (it can only shrink).
- Update the documentation in the same pull request as the behaviour: README.md.
- Tags `vMAJOR.MINOR.PATCH` build the release; the changelog comes from `git-cliff` (`cliff.toml`).
- Never commit secrets. CI scans for them; a finding is a blocker, not noise.
