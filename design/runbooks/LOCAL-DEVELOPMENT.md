# Local Development

```bash
cp .env.example .env
docker compose -f compose.dev.yaml up -d postgres
./scripts/bootstrap.sh
cargo run -p melodee-server
cargo run -p melodee-cli -- plan
```

The bootstrap server intentionally exposes only health/system-info endpoints. Broad API/domain work is
blocked by Phase 0 evidence and ADR gates. Local captures are written beneath
`compatibility/captures/` and ignored by Git.
