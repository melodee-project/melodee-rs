# Admin UI

The Carbon-based React administrative application is served by `melodee-server` at `/admin`.
Its source lives in `admin-ui/src`, with Vitest unit coverage alongside the components and
Playwright end-to-end coverage in `admin-ui/e2e`.

Use the pinned pnpm version from `package.json` and keep the generated `admin-ui/dist` output out of
version control. The canonical design rules remain in `design/ui/`.
