# Contributing to ThermalIQ

Thank you for contributing. Please read this guide before opening a PR.

## Prerequisites

- **Rust stable** — install via [rustup](https://rustup.rs/)
- No other system dependencies are required for a mock-hardware build

## Building

```sh
cargo build --features mock-hardware
```

## Testing

```sh
cargo test --features mock-hardware
```

All CI tests run with `--features mock-hardware` (see below).

## Why `mock-hardware`?

ThermalIQ reads real hardware sensors (CPU temperatures, fan RPM) via platform
HALs that only work on physical machines with appropriate drivers. The
`mock-hardware` feature replaces those HAL calls with deterministic stubs so
that the full test suite can run in CI containers, VMs, and on machines without
supported hardware. Any code that touches the sensor or fan HAL must be gated
behind this feature flag.

## Commit Format

```
<type>: <short description>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`

Example: `feat: add hysteresis to FanCurve`

## PR Process

1. Fork the repository and create a branch from `main`.
2. Make your changes, ensuring `cargo test --features mock-hardware` passes.
3. Run `cargo clippy -- -D warnings` and resolve all warnings.
4. Update the `[Unreleased]` section of `CHANGELOG.md`.
5. Open a PR against `main` and fill in the PR template.

## Hardware Testing

The platform HAL (real sensor reads and fan control) requires physical hardware
and appropriate OS privileges. If your change touches HAL code, test it on real
hardware if possible and note the hardware/OS in the PR description. If hardware
is unavailable, mark the PR as **untested on real hardware** — a maintainer will
validate before merging.
