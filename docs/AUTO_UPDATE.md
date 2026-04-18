# ThermalIQ Auto-Update Guide

ThermalIQ uses the [Tauri updater plugin](https://tauri.app/plugin/updater/) to deliver
post-launch patches. Updates are **never applied without explicit user consent** — the app
checks for a newer version, shows a non-blocking banner, and only downloads/installs when
the user clicks "Install now".

---

## How it works

1. On startup (throttled to once per 24 h) the app calls the `check_update_info` Rust
   command, which queries the GitHub Releases endpoint for `latest.json`.
2. If a newer version is available a banner appears in the bottom-right corner with:
   - "View changelog" — opens the GitHub Release page in the browser.
   - "Install now" — downloads, verifies the signature, installs, and restarts.
   - "Later" — dismisses the banner until the next 24 h window.
3. A manual "Check for updates" button is always available in **Settings → Updates**.

---

## Generating signing keys

Run once to create a keypair:

```bash
npx @tauri-apps/cli signer generate -w ~/.tauri/thermaliq.key
```

This outputs:
- **Private key** (`~/.tauri/thermaliq.key`) — keep secret, never commit.
- **Public key** (printed to stdout) — paste into `tauri.conf.json` and into the
  `TAURI_SIGNING_PUBLIC_KEY` environment variable for local dev builds.

---

## Configuring the public key

In `thermaliq-ui/src-tauri/tauri.conf.json` the `plugins.updater.pubkey` field is set to
`"$TAURI_SIGNING_PUBLIC_KEY"`. Tauri resolves this at build time from the environment
variable. For **development** (no signing key set) the updater is a no-op — unsigned dev
builds will never prompt for updates.

To test locally with signing enabled:

```bash
export TAURI_SIGNING_PUBLIC_KEY="<paste public key here>"
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/thermaliq.key)"
npm run tauri build
```

---

## Storing the private key as a GitHub secret

1. Go to **Settings → Secrets and variables → Actions** in the `lulacoding/thermaliq` repo.
2. Add secret `TAURI_SIGNING_PRIVATE_KEY` with the contents of `~/.tauri/thermaliq.key`.
3. If the key has a passphrase, also add `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Add secret `TAURI_SIGNING_PUBLIC_KEY` with the public key string.

The release workflow reads these secrets and passes them to `tauri-action`. The
`latest.json` generation step is gated on `TAURI_SIGNING_PRIVATE_KEY` being present, so
fork PRs and unsigned CI runs safely skip it.

---

## The `latest.json` manifest

After a signed release build, the workflow generates `latest.json` and uploads it
alongside the installers. It follows the
[Tauri update manifest format](https://tauri.app/plugin/updater/#update-server):

```json
{
  "version": "1.0.1",
  "notes": "See https://github.com/lulacoding/thermaliq/releases/tag/v1.0.1",
  "pub_date": "2026-05-20T12:00:00Z",
  "platforms": {
    "windows-x86_64": { "signature": "...", "url": "...ThermalIQ_1.0.1_x64-setup.exe" },
    "darwin-aarch64":  { "signature": "...", "url": "...ThermalIQ_1.0.1_aarch64.dmg" },
    "darwin-x86_64":   { "signature": "...", "url": "...ThermalIQ_1.0.1_x64.dmg" },
    "linux-x86_64":    { "signature": "...", "url": "...thermaliq_1.0.1_amd64.AppImage" }
  }
}
```

The `endpoints` URL in `tauri.conf.json` points to:

```
https://github.com/lulacoding/thermaliq/releases/latest/download/latest.json
```

GitHub automatically serves the latest non-draft release's assets at this URL.

---

## Verifying a signed release

After a release is published, confirm the manifest is reachable and well-formed:

```bash
curl -fsSL https://github.com/lulacoding/thermaliq/releases/latest/download/latest.json | jq .
```

To verify a signature manually (replace paths as needed):

```bash
npx @tauri-apps/cli signer verify \
  --pub-key "<your public key>" \
  ThermalIQ_1.0.1_x64-setup.exe \
  ThermalIQ_1.0.1_x64-setup.exe.sig
```

---

## macOS — privileged helper across updates

The `SMJobBless` privileged helper (`com.thermaliq.helper`) is installed once during first
launch and **persists across app updates**. The updater does not re-run `SMJobBless`.

If the helper binary version changes between releases, ThermalIQ detects staleness on the
next fan-write attempt and prompts the user to re-install the helper. No special handling
is required in the update flow.

---

## Linux — AppImage update strategy (deferred)

AppImage delta updates via the Tauri updater require the app to be distributed as a
`.AppImage.tar.gz` with a `.sig` sidecar. The release workflow includes the Linux entry in
`latest.json` but **Linux auto-update is not validated for v1.0**. Users on Linux should
update by downloading the new AppImage from the GitHub Releases page.

This will be revisited post-v1.0 once AppImage distribution is confirmed stable.
