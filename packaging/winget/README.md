# ThermalIQ — winget Manifest

This directory contains the [winget](https://learn.microsoft.com/en-us/windows/package-manager/winget/)
package manifests for ThermalIQ.

## Automatic generation

The release workflow (`release.yml`) generates an updated manifest set for every
non-prerelease tag and uploads it as a `winget-manifests-<version>.tar.gz` artifact
on the GitHub Release. The workflow **hard-fails** if the NSIS installer artifact is
not present — no manifest with a placeholder SHA256 will ever be emitted.

## Manual submission to winget-pkgs

The manifests are **not** submitted automatically. To publish a new version to the
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) community repository,
a maintainer must:

1. Download `winget-manifests-<version>.tar.gz` from the GitHub Release assets.
2. Extract it: `tar xzf winget-manifests-<version>.tar.gz`
3. Install [wingetcreate](https://github.com/microsoft/winget-create):
   ```powershell
   winget install Microsoft.WingetCreate
   ```
4. Submit using a GitHub PAT with `public_repo` scope stored as the
   `WINGET_PKGS_TOKEN` repository secret (or your personal token):
   ```powershell
   wingetcreate submit `
     --token <WINGET_PKGS_TOKEN> `
     l\LulaCoding\ThermalIQ\<version>
   ```
5. The wingetcreate tool will open a pull request against `microsoft/winget-pkgs`.
   Approval typically takes 1–3 business days.

Once the PR is merged, users can install via:
```powershell
winget install LulaCoding.ThermalIQ
```

## Manifest layout

```
manifests/
└── l/
    └── LulaCoding/
        └── ThermalIQ/
            └── <version>/
                ├── LulaCoding.ThermalIQ.yaml                (version manifest)
                ├── LulaCoding.ThermalIQ.installer.yaml      (installer manifest)
                └── LulaCoding.ThermalIQ.locale.en-US.yaml   (locale manifest)
```

## Schema version

All manifests target schema **1.9.0**.
