---
name: Windows PC Setup & Session Protocol
description: One-time bring-up + per-session start guide for running Claude Code on the Windows test desktop
type: reference
last_validated: 2026-04-19
---

# ThermalIQ — Windows PC Claude Code Guide

This is everything needed to drive a launch session from the Windows test desktop. Mac brain not required — the project's `.claude/context/` files are self-contained.

---

## Part 1 — One-time setup (~20 min, do once)

### 1.1 Install Claude Code

- [ ] Download Claude Code for Windows: https://claude.ai/download (or whatever the current install path is)
- [ ] Sign in with your Anthropic account
- [ ] Verify in PowerShell:
  ```powershell
  claude --version
  ```

### 1.2 Install Git + GitHub CLI

```powershell
winget install Git.Git
winget install GitHub.cli
gh auth login    # GitHub.com → HTTPS → browser
git config --global user.name "Your Name"
git config --global user.email "you@example.com"
```

### 1.3 Clone the repo

```powershell
mkdir C:\dev
cd C:\dev
git clone https://github.com/lulacoding/thermaliq.git
cd thermaliq
git pull   # always pull before each session — Mac may have pushed updates
```

### 1.4 (Optional but recommended) — Add context7 MCP

Gives Claude fast access to Tauri / NVML / Rust crate docs.

- [ ] Edit `%USERPROFILE%\.claude.json` (create if missing). Add:
  ```json
  {
    "mcpServers": {
      "context7": {
        "command": "npx",
        "args": ["-y", "@upstash/context7-mcp"]
      }
    }
  }
  ```
- [ ] Skip every other MCP server. None needed for these sessions.

### 1.5 Smoke test Claude Code from inside the repo

```powershell
cd C:\dev\thermaliq
claude
```

In the Claude Code prompt, paste:
```
read docs/launch/sessions/INDEX.md and tell me which sessions exist
```

Expected: Claude lists the 11 sessions. If yes — setup is good. Quit (`/exit`).

---

## Part 2 — Per-session protocol

Do this every time you sit down to run a session.

### 2.1 Pre-flight (2 min)

```powershell
cd C:\dev\thermaliq
git pull                         # get latest .claude/context/ updates from Mac
git status                       # should be clean
claude
```

### 2.2 The opening prompt

**Copy-paste this verbatim**, replacing `<N>` with the session number you're starting:

```
I'm starting Session <N> for the ThermalIQ v1.0 launch.

I'm on Windows (the test desktop). You don't have my Mac brain/memory loaded — work from the repo files only.

Please:
1. Read docs/launch/sessions/<NN>-*.md (the file for this session)
2. Read docs/launch/launch-plan.md for the overall arc
3. Read .claude/context/status.md and .claude/context/lessons.md if present (Mac-managed; may not exist on Windows yet — proceed without them)
4. Walk me through the session step-by-step
5. After each step, wait for me to paste output or confirm before continuing
6. Log bugs to lessons.md as we find them
7. At session end, fill in the "Session log" block at the bottom of the session file
8. Commit and push the updates so my Mac sees them

Confirm you've read the files, then ask me when I'm ready to start step 1.
```

Replace `<N>` and `<NN>` (e.g. `Session 2` → file `02-hwinfo-real-reads.md`).

### 2.3 During the session

- **Paste command output back into Claude.** I can't see your screen.
- **Long output?** Tell me to "skip to errors" — I'll search for failures only.
- **Stuck/confused?** Say "what's the safe fallback?" — I'll point at the troubleshooting table in the session file.
- **Want to pause?** Say "pause and save state" — I'll commit the session log progress before you quit.

### 2.4 Session end protocol

**Copy-paste this** when the session's "Done when" boxes are all checked:

```
Session done. Please:
1. Update the "Session log" block at the bottom of the session file with what we did
2. Append any new bugs/learnings to .claude/context/lessons.md under a new dated heading
3. Update .claude/context/status.md if a phase milestone changed
4. Show me the diff
5. Wait for my approval, then commit + push with message "session: complete S<N>"
```

After approval, on Windows:
```powershell
git log -1   # confirm commit landed
git push     # if Claude didn't push it
```

### 2.5 Mid-session save (if you have to stop)

```
I need to stop here. Please:
1. Update the session file's log block with where we are
2. Note "PAUSED — resume at step X.Y" prominently
3. Commit + push as "session: pause S<N> at step X.Y"
```

---

## Part 3 — Sync workflow Mac ↔ Windows

Both machines work on the same repo. Discipline:

| Action | Command | When |
|--------|---------|------|
| Before any session | `git pull` | Always — first thing |
| After any commit | `git push` | Always — last thing |
| Mac and Windows might both edit | Coordinate — only one machine driving at a time | Don't run parallel sessions |

**If you forget to push on one machine and start on the other:**
- `git status` on the second machine shows nothing wrong
- You make changes
- Pushing the first machine later → merge conflict in `.claude/context/`
- Resolve manually; rare if you stick to "one machine at a time"

---

## Part 4 — What Claude Code on Windows DOES have

- ✅ Full repo file access (read/edit/commit any project file)
- ✅ Bash/PowerShell execution
- ✅ Git + gh CLI
- ✅ All 12 session files in `docs/launch/sessions/` (tracked in git)
- ✅ status.md, decisions.md, lessons.md
- ✅ ROADMAP.md, all source code, docs
- ✅ Project's `CLAUDE.md` (auto-loaded — routes to context files)
- ✅ context7 MCP (if Part 1.4 done) — for library docs

## What Claude Code on Windows DOES NOT have

- ❌ Your Mac global `~/.claude/CLAUDE.md` (Brain Master)
- ❌ MEMORY.md and per-topic memory files
- ❌ claude-mem session history (~50 obs from past work)
- ❌ MCP memory server entity graph
- ❌ episodic-memory plugin (semantic recall over conversations)
- ❌ oh-my-claudecode skills/agents
- ❌ Knowledge of other projects (SpendGuard, my-api)

**This is fine.** Sessions are checklist-driven. The repo's `.claude/context/` is the shared knowledge.

---

## Part 5 — Common Windows-specific paste prompts

### "I hit an error, help me debug"
```
I ran step X.Y. Got this output:
<paste output>

What does this mean and what should I try next?
```

### "Show me the troubleshooting for this session"
```
Look at the troubleshooting table in the current session file. My symptom is: <describe>. Which row applies?
```

### "I'm not sure if this is a bug worth filing"
```
I observed: <behavior>
Expected: <expected>
My hardware: <CPU/GPU>

Is this a real bug? What severity? Should I file a GitHub issue, and if so draft it.
```

### "Help me reproduce a beta tester's report"
```
Issue #NNN reports: <paste issue body>

Walk me through reproducing this on my hardware. Stop at each step.
```

### "Cut a beta release for me"
```
We've merged fixes for issues #X #Y #Z. Cut v1.0.0-beta.<N+1>:
1. Show me the version-bump diff
2. Wait for my approval
3. Commit, tag, push
4. Watch the GH Actions release workflow
5. Tell me when it's done and what the release URL is
```

---

## Part 6 — Emergency / fast paths

### Quick context restore (if I seem to have forgotten)

If mid-session I (Claude) seem confused or out of context:
```
Re-read docs/launch/sessions/<NN>-*.md (and .claude/context/lessons.md if it exists). Summarize where we are in 3 lines.
```

### Fan emergency (S3 only)

If a fan is stuck wrong and you need to bail:
```powershell
# In the daemon's terminal: Ctrl-C
# Then in PowerShell:
Stop-Service ThermalIQ -ErrorAction SilentlyContinue
# BIOS resumes fan control within 30s. Confirm via HWiNFO64.
```

### Rollback a bad release (S8/S10)

```
Roll back release v1.0.0 (or v1.0.0-beta.N):
1. Edit GitHub release → uncheck "Latest" → check "Pre-release"
2. Confirm latest.json no longer points to it for stable users
3. Comment in pinned issue explaining the rollback
4. Help me start a hotfix branch
```

---

## Part 7 — When to escalate to Mac

Some work is faster on Mac (where my full brain is). Switch to Mac for:

- **Architectural decisions** spanning multiple sessions (cross-session context helps)
- **Code refactors** larger than a single bug fix
- **Research** ("how have we solved similar problems before?" — Mac has claude-mem history)
- **Writing new docs** (CHANGELOG, READMEs, design docs)

Switch to Windows for:

- **Anything touching real hardware** (S1–S6 except documented Mac-only steps, S9 reproductions, S10 smoke test)
- **Tauri UI testing** (S4)
- **Service install/uninstall** (S5)

When switching machines: `git pull` on the new machine first. Always.

---

## Part 8 — Cheat sheet (print this)

```
┌─────────────────────────────────────────────────────────────┐
│ ThermalIQ Session Quickstart (Windows)                      │
├─────────────────────────────────────────────────────────────┤
│ 1. cd C:\dev\thermaliq                                      │
│ 2. git pull                                                 │
│ 3. claude                                                   │
│ 4. Paste opening prompt (Part 2.2) with session number      │
│ 5. Run commands as instructed, paste output back            │
│ 6. At end, paste closing prompt (Part 2.4)                  │
│ 7. git push (if not already done by Claude)                 │
└─────────────────────────────────────────────────────────────┘

Session files: docs/launch/sessions/INDEX.md
Launch plan:   docs/launch/launch-plan.md
Bug log:       .claude/context/lessons.md (Mac-managed; may not be on Windows)
Status:        .claude/context/status.md (Mac-managed; may not be on Windows)
Token:         %APPDATA%\thermaliq\api.token
Daemon URL:    http://127.0.0.1:7272
```

---

## TL;DR

1. Install Claude Code + git + gh (5 min, once)
2. `git clone` the repo (1 min, once)
3. Per session: `cd C:\dev\thermaliq && git pull && claude` then paste the opening prompt
4. After each session: `git push`
5. Mac and Windows share the repo via git — never run sessions in parallel
