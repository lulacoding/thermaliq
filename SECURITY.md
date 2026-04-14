# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` (pre-release) | Yes |
| Any tagged release | Yes (until superseded) |

ThermalIQ has not yet reached a stable 1.0 release. Only the `main` branch
and the most recent tagged release receive security fixes.

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Use GitHub's private vulnerability reporting:
[Security Advisories → New draft security advisory](/security/advisories/new)

We will acknowledge your report within **48 hours** and provide a triage
decision within **14 days**. If the issue is confirmed we will coordinate a fix
and disclosure timeline with you before publishing.

## Out of Scope

The following are known issues and are not eligible for a security report:

- Vulnerabilities in third-party kernel drivers used for hardware access
  (e.g. WinRing0 — see CVE-2020-14979). These are upstream issues outside our
  control. Prefer the platform-native alternatives where available.

## Safe Coding Practices for Contributors

- Do not store secrets (tokens, credentials) in source files or config files
  that ship with the binary.
- Validate and sanitise all values read from hardware registers before use —
  sensors can return out-of-range readings.
- Avoid `unsafe` Rust blocks unless strictly necessary for HAL interop; any
  `unsafe` addition requires a safety comment and reviewer sign-off.
- The HTTP API must enforce authentication on every endpoint — no unauthenticated
  routes except the health check.
- Prefer bounded channels and explicit timeouts over unbounded queues to prevent
  resource exhaustion.
