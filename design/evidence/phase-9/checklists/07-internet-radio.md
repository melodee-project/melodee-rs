# Checklist 07 — Internet radio (WP-06)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| OpenSubsonic radio CRUD (admin-gated) | PRD-RADIO-001/002, PRD-API-OS-004 | ✅ | Phase 8 `os-radio-*` differential evidence |
| Native-API radio list/read | PRD-RADIO-001 | ⬜ | WP-06: native routes over shared store (ADR-0134 §1) |
| Native-API radio create/update/delete (admin-gated) | PRD-RADIO-002 | ⬜ | WP-06: routes + authorization tests |
| Stations survive migration | PRD-RADIO-002, PRD-MIG-003 | ⬜ | WP-06: ID stability note; Phase 11 owns production import |
