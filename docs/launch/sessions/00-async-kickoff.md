---
name: Session 0 — Async Kickoff
description: Start multi-day-lead-time procurement (cert, Apple, hardware inventory) so blockers don't surface mid-launch
type: project
session: 0
target_date: 2026-04-19
duration_minutes: 10
risk: low
---

# Session 0 — Async Kickoff

## Why this session exists

Three things have multi-day wall-clock lead times that can't be compressed:
1. **EV code-signing cert identity verification** (3–5 business days at SSL.com)
2. **Apple Developer Program enrollment** (24–48h, only if shipping macOS)
3. **Hardware procurement** (if you don't already own a Windows test box)

If you start any of these on Day 20, you ship late. Start them today.

## Prerequisites

- Personal/business identity documents on hand (driver's license + utility bill or business registration)
- Credit card with ~$300 USD headroom (cert + optional Apple)
- A Windows desktop you can dedicate to testing (not your daily driver)

## Step-by-step

### 0.1 Order EV code-signing cert (SSL.com eSigner)

- [ ] Go to https://www.ssl.com/certificates/ev-code-signing/
- [ ] Pick **eSigner** variant (cloud-based, no USB key needed). Confirm during checkout that this is the cloud variant — if they try to ship a token, switch.
- [ ] Cost: ~$249/yr (1-year). Multi-year is cheaper per year but commits you longer.
- [ ] During checkout, fill in:
  - Organization legal name (must match identity docs exactly)
  - Address
  - Phone (they'll call to verify — ensure voicemail is professional)
- [ ] Pay. Save the order number.
- [ ] Watch email for "Identity verification required" — usually within 24h. Respond within 48h or order is cancelled.
- [ ] Identity verification typically requires:
  - Government ID upload
  - Notarized affidavit (some jurisdictions)
  - Phone callback (answer, confirm name, reference order number)
- [ ] **Once verified:** SSL.com gives you 4 secrets — `USERNAME`, `PASSWORD`, `CREDENTIAL_ID`, `TOTP_SECRET`. **Do not put these in Session 0**. Store them in a password manager (1Password / Bitwarden) for Session 7.

### 0.2 macOS in/out decision for v1.0

**Recommendation: OUT.** Reasons:
- macOS shipping requires SMJobBless helper signing (Phase 3.4 work, not done)
- Apple Developer enrollment + notarization adds 3–5 days of complexity
- M-series fan writes are advisory-only without SMJobBless → poor UX
- Mac-only users are a minority of the target audience (overclockers, gamers)

**Decision:**
- [ ] **Mac OUT for v1.0** → skip 0.3, document in `status.md` "Mac shipped in v1.1 (post-launch)"
- [ ] **Mac IN for v1.0** → continue to 0.3 below

### 0.3 Apple Developer Program (only if Mac IN)

- [ ] Go to https://developer.apple.com/programs/enroll/
- [ ] Cost: $99/yr (individual) or $99/yr (organization, harder enrollment)
- [ ] Requires Apple ID with 2FA enabled
- [ ] Identity verification: 24–48h
- [ ] Once enrolled, note your **Team ID** (10-char alphanumeric, e.g. `ABC123DEF4`) — needed for Session 7

### 0.4 Hardware inventory

You need to know exactly what hardware you're testing against. Surprises in S2/S3 will cost hours.

- [ ] On the Windows test desktop, run:
  ```cmd
  systeminfo > C:\hwinfo.txt
  wmic cpu get name
  wmic path win32_videocontroller get name
  wmic baseboard get product,manufacturer
  ```
- [ ] Email yourself `hwinfo.txt` (or sync via Dropbox/OneDrive)
- [ ] Mac-side prep: I'll create `.claude/context/test-hardware.md` with a template — you fill in:
  - CPU (model + TDP)
  - GPU vendor (NVIDIA/AMD/Intel) + model + driver version
  - Motherboard chipset
  - OS build (`winver`)
  - Installed fan count (look at the case)
  - PSU model (matters for sensor exposure)

### 0.5 USB key check

- [ ] If SSL.com order ships a physical USB token: order won't process until you receive + provision it (adds 5–10 days). If you got eSigner cloud variant, skip.
- [ ] If physical key arrives: install the SafeNet drivers per SSL.com's instructions — but defer activation to Session 7.

## Mac-side prep (Claude does)

I'll write the following files now or on request:
- `.claude/context/test-hardware.md` — inventory template
- A reminder note in `status.md` flagging the cert order date so we know the 3-5 day window

## Done when

- [ ] EV cert order is placed AND identity verification submitted (not just paid)
- [ ] macOS in/out decision documented in `status.md`
- [ ] If Mac IN: Apple enrollment started
- [ ] `test-hardware.md` filled with desktop specs
- [ ] Calendar reminder set for cert ETA (today + 5 business days)

## Session log

```
Date executed: 
Time taken: 
Cert order ID: 
Mac decision: IN / OUT
Apple Team ID (if applicable): 
Notes / surprises:
```

## What unblocks next

- S1 (toolchain) — unblocked immediately, no waiting
- S7 (signing) — blocked until cert verified (~3-5 business days)
- S8 (beta) — needs S7 done first
