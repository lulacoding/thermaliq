# Stage 6: Landing Page, One-Line Install UX & Public Launch Strategy
**Status:** Complete | **Tier:** MEDIUM | **Model:** Sonnet

## Reference projects studied
Oh My Zsh (~170k stars), Homebrew (~41k stars), rustup (~6k stars), FanControl/Releases (~7k stars). HN launch best practices from markepear.dev analysis of top Show HN posts.

---

## 1. ONE-LINE INSTALL COMMANDS

### macOS — Homebrew (Primary)

**After tap acceptance:**
```bash
brew install thermaliq
```

**Pre-tap (self-hosted):**
```bash
brew tap thermaliq/tap && brew install thermaliq
```

Formula must: install `thermaliq` binary, install launchd plist to `~/Library/LaunchAgents/dev.thermaliq.agent.plist` and load it, print post-install message pointing to `thermaliq init`.

Tap hosting: `github.com/thermaliq/homebrew-tap` — self-hosted, costs nothing, no review queue.

### Linux — curl (Primary)

**Security-hardened variant (match rustup's pattern):**
```bash
curl --proto '=https' --tlsv1.2 -fsSL https://thermaliq.dev/install.sh | sh
```

**install.sh — distro detection logic:**
```bash
#!/bin/sh
set -e

INSTALL_DIR="${THERMALIQ_INSTALL:-$HOME/.local/bin}"
REPO="https://github.com/thermaliq/thermaliq/releases/latest/download"

detect_arch() {
  case "$(uname -m)" in
    x86_64)  echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *)       echo "unsupported"; exit 1 ;;
  esac
}

detect_distro_package_manager() {
  if [ -f /etc/os-release ]; then
    . /etc/os-release
    case "$ID $ID_LIKE" in
      *debian*|*ubuntu*) echo "apt" ;;
      *fedora*|*rhel*|*centos*) echo "dnf" ;;
      *arch*|*manjaro*) echo "pacman" ;;
      *opensuse*) echo "zypper" ;;
      *alpine*) echo "apk" ;;
      *) echo "binary" ;;
    esac
  else
    echo "binary"
  fi
}

main() {
  OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  ARCH=$(detect_arch)
  echo "ThermalIQ installer — OS: $OS, Arch: $ARCH"
  BINARY_URL="$REPO/thermaliq-$OS-$ARCH"
  mkdir -p "$INSTALL_DIR"
  curl -fsSL "$BINARY_URL" -o "$INSTALL_DIR/thermaliq"
  chmod +x "$INSTALL_DIR/thermaliq"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "Add $INSTALL_DIR to your PATH: export PATH=\"\$PATH:$INSTALL_DIR\"" ;;
  esac
  if command -v systemctl >/dev/null 2>&1; then
    "$INSTALL_DIR/thermaliq" install-service --user
  fi
  echo "Run: thermaliq init"
}
main "$@"
```

Key design decisions: binary download (no Rust toolchain required), `--proto '=https' --tlsv1.2` (TLS enforcement), `set -e` (stop on error), respects `THERMALIQ_INSTALL` env var, no root required.

### Windows — winget (Primary)
```powershell
winget install thermaliq.thermaliq
```

**Scoop (power user preference):**
```powershell
scoop bucket add thermaliq https://github.com/thermaliq/scoop-bucket
scoop install thermaliq
```

**PowerShell bootstrapper (users without winget):**
```powershell
irm https://thermaliq.dev/install.ps1 | iex
```

`install.ps1` pattern: check winget → use it; else check Scoop → use it; else download MSI from GitHub releases.

### Universal — cargo (Secondary, Rust devs only)
```bash
cargo install thermaliq        # compiles from source, ~5 min, requires Rust toolchain
cargo binstall thermaliq       # downloads pre-built binary — strictly better UX
```

Label clearly as "for Rust developers" — NOT a general-purpose install method.

**Note:** winget submission requires Microsoft review (1-2 weeks). Homebrew core acceptance requires download thresholds. Ship tap/bucket approach first.

---

## 2. LANDING PAGE STRUCTURE — thermaliq.dev

**Key finding:** Pages that bury the install command below the fold lose ~40% of developer conversions at first visit.

### Section 1 — Hero (above the fold, no scroll)

```
[Logo] ThermalIQ

[H1] Fan control that thinks for itself.

[H2] An AI agent that monitors your thermals 24/7, learns your workloads,
     and auto-tunes every fan curve — no manual tweaking required.

[CTA Primary]   → Install on macOS
[CTA Secondary] → View on GitHub   [Star count badge]

[Visual] Terminal animation:
  $ thermaliq watch
  [14:22:01] CPU 67°C — gaming load detected
  [14:22:01] Applying profile: gaming-high-perf
  [14:22:04] Fan 1: 1200 → 2100 RPM  Fan 2: 900 → 1800 RPM
  [14:22:30] Temps stabilised at 61°C (↓6°C in 29s)
```

Use auto-playing asciinema cast or custom SVG animation — no video (too heavy for first load).

### Section 2 — Install Block (OS-tabbed, copy button on each)

```
[ macOS ] [ Linux ] [ Windows ] [ Cargo ]
```

Auto-detect OS via `navigator.platform` / `navigator.userAgent`. Show "Detected: macOS (Apple Silicon)" with override option.

### Section 3 — The Problem

```
[H2] Fan curves shouldn't be a full-time job.

Manual fan control means:
✗ Tuning curves per-game, per-workload, per-season
✗ Fans screaming on 5-second CPU spikes
✗ Silent fans when you actually need cooling
✗ Windows-only tools with no Linux/macOS support
```

### Section 4 — AI Agent Feature Cards (three-column)

```
[Card 1] 24/7 Autonomous Agent
The agent watches every sensor continuously.
When thermals shift, it acts — you don't have to.

[Card 2] Learns Your Workloads
Games, compiles, renders, idle — ThermalIQ identifies
patterns and pre-positions fans before temperatures spike.

[Card 3] Explains Every Decision
Every fan change is logged with a reason.
No black box. Full audit trail in plain text.
```

### Section 5 — Technical Credibility Block

```
✓ Full FanControl parity on Windows
✓ Linux: hwmon/lm-sensors, systemd service
✓ macOS: SMC access via Apple Silicon API
✓ TOML profiles — git-diff friendly, human-readable
✓ Claude Code SKILL.md integration
✓ OpenClaw agent framework compatible
✓ MIT license — no telemetry, no accounts, no cloud
✓ Zero runtime dependencies
```

### Section 6 — Live Demo
Option A (preferred): Embedded asciinema cast — `thermaliq watch`, load spike, agent detects and adjusts RPMs, log line explaining decision. ~45 seconds, autoplay muted, loop.
Option B: Screenshot carousel.

### Section 7 — Comparison Table

```
                    | ThermalIQ | FanControl | Fan2    | lm_sensors
--------------------|-----------|------------|---------|------------
Windows             |     ✓     |     ✓      |    ✗    |     ✗
Linux               |     ✓     |     ✗      |    ✓    |     ✓
macOS               |     ✓     |     ✗      |    ✗    |     ✗
AI agent            |     ✓     |     ✗      |    ✗    |     ✗
Git-friendly config |     ✓     |     ✗      |    ✗    |     ✗
MIT license         |     ✓     |  Source     |   GPL   |    GPL
No telemetry        |     ✓     |     ✓      |    ✓    |     ✓
CLI-first           |     ✓     |     ✗      |    ✗    |     ✓
```

Must be factually accurate at launch — FanControl's license is currently source-available, not MIT. Verify before publishing.

### Section 8 — TOML Profile Showcase

```toml
# ~/.config/thermaliq/profiles/gaming.toml
[profile]
name = "gaming-high-perf"
trigger = { workload = "gaming", cpu_temp_gt = 60 }

[[fans]]
id = "cpu_fan"
curve = { type = "linear", min_rpm = 800, max_rpm = 2400 }

[[fans]]
id = "case_front"
curve = { type = "step", steps = [
  { temp = 50, rpm = 600 },
  { temp = 65, rpm = 1500 },
  { temp = 75, rpm = 2200 },
]}
```

### Section 9 — Community / Credibility
GitHub stars badge, License: MIT badge, platform badges. GitHub link + GitHub Discussions or Discord link.

### Section 10 — Footer
```
MIT License  ·  No telemetry  ·  No accounts  ·  No cloud
[GitHub] [Docs] [Changelog] [Contributing]
thermaliq.dev — Astro + Starlight, deployed on Cloudflare Pages
```

---

## 3. HERO COPY ALTERNATIVES

Developer tool copy performs best when the first line is a direct statement of what the tool does. Superlatives underperform concrete specifics on HN/Reddit.

| Variant | Headline | Best for |
|---------|----------|----------|
| 1 — Technical | `Autonomous fan control with a 24/7 learning agent.` | Default/HN |
| 2 — Gamer | `Stop babysitting your fans. Let the AI handle it.` | Gaming subreddits |
| 3 — Linux | `A fan controller that actually works on Linux.` | r/linux, HN |
| 4 — General | `Fan control that thinks for itself.` | Default landing page |
| 5 — AI | `The first fan controller with a real AI agent inside.` | AI-adjacent audiences |

**Recommendation:** Variant 4 as default. A/B test Variant 3 on r/linux / HN. Variant 2 for gamer-audience paid promotion.

---

## 4. SHOW HN POST

### Title (70 chars — within limit)
```
Show HN: ThermalIQ – AI agent fan controller for Windows, Linux, macOS
```

Alternative (76 chars):
```
Show HN: ThermalIQ – open-source fan controller with a 24/7 learning agent
```

### First Comment (post yourself, immediately after submission)

```
Hey HN, author here.

ThermalIQ is an MIT-licensed fan controller that runs an autonomous agent
in the background. The agent is stateful: it maintains a rolling thermal
model of your system, classifies workloads (gaming, compile, render, idle)
using sensor heuristics, and rebalances fan curves when it detects a shift.
All decisions are logged with a reason string so you can audit what happened.

**Why not just use FanControl?**
FanControl is Windows-only and GUI-first. ThermalIQ targets all three
platforms with a CLI-first, TOML-based config. Profiles are plain text
and git-diffable — your fan config lives in your dotfiles.

**Why not just write a bash script with lm-sensors?**
You can. ThermalIQ does the sensor polling, workload classification, and
fan curve interpolation for you, with hysteresis handling to prevent RPM
hunting. It's the plumbing most bash scripts skip.

**What does "AI agent" mean here — is it an LLM?**
No LLM required for basic operation. The agent is a control loop with a
thermal model and workload classifier. The "AI" integration is the optional
Claude Code SKILL.md, which lets you configure profiles and query the agent
via natural language in your Claude Code session. The agent itself is
deterministic.

**Is piping curl to sh safe?**
The install script is on GitHub and linked directly from thermaliq.dev.
It downloads a pre-built binary and optionally installs a systemd user
service. No root required. You can audit the script before running it:
curl -fsSL https://thermaliq.dev/install.sh | less

**Platform status:**
- Windows: stable (full FanControl parity)
- Linux: stable (hwmon/lm-sensors/systemd)
- macOS: beta (SMC access, Apple Silicon primary)

Feedback welcome — especially from Linux users with exotic hardware.
```

---

## 5. REDDIT LAUNCH STRATEGY

Developer tool Reddit launches perform best with problem-solution framing, not product name. Subreddit choice matters more than post quality.

### Tier 1 — Post First (Highest relevance)

**r/linux (~1.1M):** Text post. "I built an AI agent fan controller that actually works on Linux (MIT, CLI-first, TOML config)." Lead with Linux pain point (no FanControl, lm-sensors is tedious). Show install command, real TOML profile, GitHub link. Under 300 words.

**r/pcmasterrace (~10M):** Image/GIF post showing thermals before/after. Lead with gaming performance (Windows-primary audience — do NOT lead with Linux support).

**r/homelab (~1.8M):** Text post. Emphasise headless operation, systemd service, TOML in dotfiles, no GUI dependency.

### Tier 2 — Post 2-3 days after Tier 1

**r/rust (~300k):** Link to GitHub, invite contributors, talk about HAL and sensor abstraction.

**r/MacOS (~600k):** Be honest about beta status. Don't oversell.

**r/sysadmin (~900k):** Focus on ops use case — fleet deployment, TOML config management, no GUI, CLI only.

**Skip r/artificial** — audience mismatch. "AI" in ThermalIQ is a control-loop agent, not an LLM product. Will generate confusion.

### Tier 3 — Only after traction
r/selfhosted, r/unixporn (show terminal setup with thermaliq watch running)

### What NOT to say anywhere
- "This is the best fan controller ever built"
- "I finally fixed [problem]" (dismissive of existing solutions)
- "Powered by AI" without immediately explaining what that means
- Asking for upvotes in any form
- Cross-posting identical text simultaneously (wait 48h between posts, adjust framing)

---

## 6. DISCORD vs. GITHUB DISCUSSIONS

**Recommendation: GitHub Discussions at launch, Discord at 500+ stars.**

GitHub Discussions is searchable, indexed, doesn't require an account for read access. Discord enables casual questions but content is siloed and disappears.

### GitHub Discussions Categories (launch configuration)
```
📣 Announcements     (maintainer-only posting, community replies)
💬 General           (open discussion)
🙋 Q&A               (support questions — mark answers, searchable)
💡 Ideas             (feature requests with structured template)
🐛 Bug Reports       (redirect to Issues for reproducibles)
🔧 Profiles          (share your TOML profiles, show your setup)
🤝 Contributing      (for new contributors, PR discussion)
```

### Discord channels (when applicable, post 500 stars)
```
#announcements (maintainer-only)
#general
#support-windows / #support-linux / #support-macos
#profiles (share configs)
#ai-agent (agent discussion, SKILL.md usage)
#dev (contributing, architecture)
#showcase (show your temps, your setup)
```

---

## 7. DOCUMENTATION STRUCTURE (Starlight)

### Sidebar structure
```
Getting Started
  ├── Installation          ← Platform tabs, one-liner commands
  ├── Quick Start           ← thermaliq init → first profile → thermaliq watch (5 min)
  └── How the Agent Works   ← Conceptual overview, not reference

Guides
  ├── Creating Profiles     ← TOML authoring guide with examples
  ├── Workload Detection    ← How the classifier works, how to tune it
  ├── Using the CLI         ← Day-to-day commands
  └── Migrating from FanControl ← Import existing curves

Platform Guides
  ├── Windows               ← Sensor backends, WMI, service install
  ├── Linux                 ← hwmon paths, lm-sensors, systemd user service
  └── macOS                 ← SMC access, launchd, Apple Silicon specifics

Reference
  ├── CLI Reference         ← Every command, flag, exit code (auto-generated)
  ├── Profile Schema        ← Complete TOML reference with types and defaults
  ├── Agent Decisions       ← Log format, decision fields, audit trail structure
  └── Environment Variables ← THERMALIQ_CONFIG, THERMALIQ_LOG_LEVEL, etc.

Integrations
  ├── Claude Code (SKILL.md)  ← How to use the skill, example prompts
  ├── OpenClaw               ← Agent framework integration guide
  └── Home Assistant         ← (future — placeholder with issue link)

Contributing
  ├── Development Setup
  ├── Hardware Abstraction Layer  ← How to add new sensor backends
  ├── Writing Tests
  └── Release Process

FAQ                          ← Flat page, question anchors, no sidebar nesting
```

### Priority-zero doc at launch
**Hardware compatibility matrix:** Table of confirmed-working hardware (motherboard/fan controller chipsets) with contributor's OS and kernel version. Builds trust faster than any feature description.

### "5-minute win" Quick Start structure
```markdown
## 1. Install (30 seconds) — [platform tabs with one-liners]
## 2. Initialise (1 minute)
$ thermaliq init   # detects fans and sensors, generates starter profile
## 3. Watch the agent run (1 minute)
$ thermaliq watch   # live sensor readings and agent decisions
## 4. Edit your first profile (2 minutes)
$ thermaliq profile edit default   # opens in $EDITOR
## 5. Apply and verify
$ thermaliq profile apply default && thermaliq status
```

---

## 8. README.md STRUCTURE

GitHub renders READMEs ~800px wide. First ~15 lines are above the fold on 1080p. Install command must be in lines 1-15.

### Recommended section order
```markdown
# ThermalIQ
> Fan control that thinks for itself.

[![License: MIT][badge]] [![Stars][badge]] [![CI][badge]] [![Platform][badge]] [![Crates.io][badge]]

A cross-platform AI agent fan controller. [2-sentence description]

## Install
[Platform-tabbed install commands — macOS/Linux/Windows/Cargo]
Then: `thermaliq init`

## Demo
[GIF or asciinema embed — 15-20 seconds, autoplay]

## Features
[Bullet list — 7 key features]

## Quick Start
[5-step sequence: init → watch → edit → apply → status]

## Documentation
[Links to docs sections]

## Comparison
[Feature table vs FanControl, lm-sensors]

## Contributing
[Link to CONTRIBUTING.md, focus areas]

## License
MIT — see LICENSE
```

### Demo GIF guidance
- Record with asciinema, export to GIF with `agg` (Rust-based converter, small file size)
- Target: under 2MB
- Show value (agent detecting load and adjusting RPMs), NOT installation
- 15-20 seconds, loop seamlessly

### Badges to include
```markdown
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/thermaliq/thermaliq/actions/workflows/ci.yml/badge.svg)](...)
[![Crates.io](https://img.shields.io/crates/v/thermaliq.svg)](https://crates.io/crates/thermaliq)
[![GitHub Stars](https://img.shields.io/github/stars/thermaliq/thermaliq?style=social)](...)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)]()
```

Do NOT add: code coverage badge, "built with" badges, Discord member count.
