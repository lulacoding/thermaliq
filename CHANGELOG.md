# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `mock-hardware` feature flag for CI and hardware-free development
- Bearer token authentication for the HTTP API
- SQLite-backed sensor history with configurable retention
- Temperature alert system with configurable thresholds and cooldown
- `FanCurve` with hysteresis, Start%, and Stop% parameters
- Agent state machine skeleton for lifecycle management
- CI/CD pipeline with GitHub Actions (Linux, Windows, macOS matrix)

### Fixed
- macOS temperature sensor spec: corrected `Ftst` to `Ftst=1`
