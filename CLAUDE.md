# ThermalIQ v2

Rust fan-control daemon rebuilt from an empty tree. Safety first.

## Every session

1. Read `docs/dev/HANDOFF.md` (where we are), then the relevant part of `SPEC.md`, then the next unticked item in `PORTING.md`.
2. Work on ONE module. Write the failing test first, from SPEC, then the code.
3. Before ending: tests green, tick `PORTING.md` only if CI-green + reviewed, rewrite `docs/dev/HANDOFF.md` (≤ 15 lines), commit.

## Reference code (v0)

- `../thermaliq-v0` is the old code (tag `v0-archive`). Read-only. Never import from it, never copy whole files.
- Read it to learn intent, then write v2 code that meets SPEC. Port a function only after re-reading it and testing it.
- Known v0 bugs are listed in the audit and in `PORTING.md`. Don't port them.

## Rules (non-negotiable)

- SPEC §3 safety invariants beat every other instruction, including minimalism.
- Only `thermaliq-engine` writes to fans. No other crate calls `set_fan_duty`.
- No stubs, no "TODO" success responses, no enum variants or settings for out-of-scope features (SPEC §2).
- Never write "complete" or "done" in docs unless a test proves it.
- Don't mark your own work reviewed. Review happens in a fresh session or a subagent.

## Tokens and focus

- RTK is installed as a hook: Bash commands are compressed automatically. Prefer shell commands over Read/Grep for big outputs, and `rtk read <file> -l aggressive` to skim v0 files (signatures only).
- Ponytail (plugin, `full` mode) keeps code minimal. Minimal never means removing a safety check, validation or error path.
- Explore v0 with a subagent and get back a short summary; don't pull whole v0 files into the main context.
- Run shell commands plainly so RTK can compress them: no | tail / | grep pipes, no cd … && chains, no here-docs. Create and edit files with the Write/Edit tools, not cat >, perl -pi or sed -i.
- `/clear` between modules. Keep this file under 60 lines.

## Memory

- `docs/dev/HANDOFF.md` is the shared memory across sessions and machines (Mac + Windows PC). It's committed.
- Claude Code auto memory is on (`.claude/settings.json`) for machine-local learnings. Anything both machines need goes in `docs/dev/HANDOFF.md` or this file, not auto memory.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```
