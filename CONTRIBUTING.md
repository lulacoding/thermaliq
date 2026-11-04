# Contributing

ThermalIQ v2 is a Rust fan-control daemon. Fan control affects cooling, so safety comes before everything else, including minimalism.

## Build and test

```bash
scripts/dev-setup.sh      # macOS / Linux
scripts/dev-setup.ps1     # Windows

cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All four must pass before you open a PR. CI runs them on Ubuntu, Windows and macOS.

## Workflow

Work proceeds one module at a time, following `PORTING.md`.

1. Read `docs/dev/HANDOFF.md` (where we are), the relevant part of `SPEC.md`, then the next unticked item in `PORTING.md`.
2. Work on one module. Write the failing test first, from SPEC, then the code.
3. Open a PR with the module name in the title. Leave the `PORTING.md` box unticked in that PR.
4. A reviewer other than the author checks the PR in a fresh session. Authors do not mark their own work reviewed.
5. Once the PR is CI-green and reviewed, the box is ticked in a separate commit.

Rules:

- Only `thermaliq-engine` writes to fans. No other crate calls `set_fan_duty`.
- No stubs, "TODO" success responses, or enum variants and settings for out-of-scope features (SPEC §2).
- Don't write "complete" or "done" in docs unless a test proves it.
- The old v0 code (tag `v0-archive`) is for reading intent only. Don't copy whole files or port known v0 bugs.

## Safety PRs

The SPEC §3 safety invariants beat every other rule. Any PR that touches a §3 rule needs a test for every §3 rule it touches. A PR that changes safety behaviour without those tests will not be merged.
