# Security Policy

## Reporting a problem

Please report problems privately through GitHub security advisories: go to the **Security** tab of this repository and choose **Report a vulnerability**. Don't open a public issue for security or safety problems.

## What counts

ThermalIQ controls fans, so a failure can let hardware overheat. **Any safety bug counts as a security issue.** That includes anything that breaks a SPEC §3 safety invariant, for example:

- fans not going to a safe duty, or not being returned to automatic control, when sensors fail, the daemon stops, or the process crashes
- a way for an unprivileged user or remote party to set fan speeds, change the config or profiles, or read the control token

Ordinary bugs and hardware compatibility problems go through the normal issue templates.
