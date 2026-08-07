# Create and Push the GitHub Repository

The ZIP intentionally does not contain a `.git` directory. After extracting it:

```bash
cd melodee-rs
git init -b main
git add .
git commit -m "chore: initialize Melodee Rust rewrite"
```

Using GitHub CLI:

```bash
gh auth status
gh repo create melodee-project/melodee-rs \
  --public \
  --source=. \
  --remote=origin \
  --push
```

Change the owner, repository name, and visibility as desired. The rewrite can also live in a private
repository during development even though the intended source and required dependency stack are FOSS.

## Recommended repository settings

- Protect `main`.
- Require the `CI / Validate repository and Rust workspace` check.
- Require at least one review for ADR, compatibility, schema, auth, destructive-policy, and migration
  changes.
- Disable force pushes and branch deletion on `main`.
- Enable secret scanning and dependency alerts where available.
- Use issues from `design/agents/FIRST-10-ISSUES.md` for the initial backlog.
- Add labels for `phase-0`, `adr`, `compatibility`, `performance`, `security`, `migration`, and each API
  surface.

## First commands after clone

```bash
cp .env.example .env
docker compose -f compose.dev.yaml up -d postgres
./scripts/bootstrap.sh
```

The first dependency-resolving run generates `Cargo.lock`; commit it immediately.
