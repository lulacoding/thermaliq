# macOS Packaging

## Files

- `embed_helper.sh` — post-build script that patches Info.plist versions and embeds the launchd plist into the helper binary's `__TEXT,__info_plist` section.
- `com.thermaliq.helper.plist` — launchd plist for the privileged helper (copy of canonical source; see below).

## Canonical launchd plist

**Canonical source:** `thermaliq-ui/src-tauri/com.thermaliq.helper.plist`

The copy in this directory exists for packaging scripts that run outside the Tauri source tree. Always edit the canonical source first, then mirror changes here.

## TODO(release) — Apple Developer Team ID

Before creating a notarized release build you MUST replace the `REPLACE_WITH_TEAM_ID` placeholder in **both** of the following files with your real 10-character Apple Developer Team ID (e.g. `AB12CD34EF`):

1. `thermaliq-helper/Info.plist` — `SMAuthorizedClients` array value
2. `thermaliq-ui/src-tauri/Info.plist` — `SMPrivilegedExecutables` dict value

The `embed_helper.sh` script performs this substitution automatically when `APPLE_TEAM_ID` is exported in the environment:

```bash
export APPLE_TEAM_ID=AB12CD34EF
./packaging/macos/embed_helper.sh
```

Failure to set a real Team ID will cause `SMJobBless` to reject the helper at runtime on notarized builds. The build will succeed but installation will fail silently.

### Verifying the substitution

```bash
# Should print your real Team ID, not REPLACE_WITH_TEAM_ID
grep -r "REPLACE_WITH_TEAM_ID" thermaliq-helper/Info.plist thermaliq-ui/src-tauri/Info.plist \
  && echo "ERROR: Team ID placeholder not replaced" && exit 1
```

Add this check to your CI release pipeline before notarization.
