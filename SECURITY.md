# Security Policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub Security Advisories for the
repository rather than opening a public issue. Include the affected version/commit, reproduction
steps, impact, and any proposed mitigation. Do not include real passwords, tokens, API keys, salts,
private media paths, or personally identifying library data in reports.

## Security-sensitive areas

Changes involving authentication, refresh tokens, HMAC stream URLs, path handling, archives,
uploads, external metadata URLs, scripting, FFmpeg process invocation, or migration tooling require
an explicit security review.

## Baseline rules

- Secrets never enter source control, fixtures, logs, screenshots, or generated compatibility data.
- Paths from users or external metadata are canonicalized and constrained to configured roots.
- Child-process arguments are passed as structured arguments, never shell-concatenated strings.
- Public errors are safe; internal logs retain correlation and root-cause detail without secrets.
- Dependencies must pass advisory and license checks.
- Destructive operations require authorization, durable audit records, and clear operator intent.
