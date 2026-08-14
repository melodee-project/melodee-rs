-- Phase 9 scripting hooks: operator script registry and execution audit
-- (ADR-0134 §6, ADR-0135, PRD-SCRIPT-001..004).
--
-- Additive. A binary downgrade leaves both tables intact; schema rollback
-- restores a verified pre-Phase-9 backup into a new database.
--
-- Nothing is seeded: scripts are operator-installed only (ADR-0135 operator
-- trust boundary); there is no default script content. The registry replaces
-- the legacy Settings-table `script.{eventName}` JSON documents (evidence:
-- design/evidence/phase-9/script-events-legacy-inventory.md §3); the audit
-- table closes the legacy "log lines only" gap (inventory §2).

CREATE TABLE script_registry (
    id uuid PRIMARY KEY,
    -- Hook event name (e.g. directoryProcessingStart). camelCase legacy
    -- contract; never an identity outside this table.
    event text NOT NULL,
    -- Operator handle within the event; stable upsert key together with
    -- event. Path-hostile characters are excluded so admin routes keyed by
    -- (event, name) stay unambiguous.
    name text NOT NULL,
    -- Script body (bare expression or `function check` form; contract in
    -- ADR-0135 and the migration guide). Bounded at 256 KiB.
    source text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    -- Wall-clock budget per execution. ADR-0135 §3 hard ceiling: 10_000 ms.
    timeout_ms integer NOT NULL DEFAULT 250,
    -- Advisory memory budget. Boa cannot enforce a heap limit (ADR-0135 §3:
    -- memory is bounded indirectly via payload/return caps plus the circuit
    -- breaker); the value is recorded for forward compatibility and sanity
    -- bounds only (4 KiB..64 MiB).
    memory_bytes bigint NOT NULL DEFAULT 16777216,
    -- Bumped when the stored source changes so operators can see script churn.
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT script_registry_event_check CHECK (
        btrim(event) <> '' AND char_length(event) <= 100
    ),
    CONSTRAINT script_registry_name_check CHECK (
        btrim(name) <> '' AND char_length(name) <= 255
        AND name !~ '[/[:cntrl:]]'
    ),
    CONSTRAINT script_registry_source_check CHECK (
        btrim(source) <> '' AND octet_length(source) <= 262144
    ),
    CONSTRAINT script_registry_timeout_check CHECK (
        timeout_ms BETWEEN 1 AND 10000
    ),
    CONSTRAINT script_registry_memory_check CHECK (
        memory_bytes BETWEEN 4096 AND 67108864
    ),
    CONSTRAINT script_registry_version_check CHECK (version >= 1),
    CONSTRAINT script_registry_event_name_unique UNIQUE (event, name)
);

-- Bounded operator listing and the per-event enabled lookup the hook gate
-- runs on every claimed candidate directory.
CREATE INDEX script_registry_event_enabled
    ON script_registry (event) WHERE enabled;

-- Append-only execution audit (ADR-0134 §6, ADR-0135 §4): one row per
-- script execution, never updated, purged only by bounded retention
-- (`ScriptRunStore::purge_older_than`). `script_id` survives script deletion
-- (ON DELETE SET NULL) so the audit outlives registry churn; ad-hoc admin
-- test-runs are recorded with script_id NULL.
CREATE TABLE script_runs (
    id uuid PRIMARY KEY,
    script_id uuid REFERENCES script_registry (id) ON DELETE SET NULL,
    event text NOT NULL,
    -- allow / deny are real script verdicts; error / timeout are
    -- fail-degraded defaults (the host operation proceeded as allow);
    -- tripped records the circuit-breaker disabling a script.
    outcome text NOT NULL,
    duration_ms bigint NOT NULL DEFAULT 0,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT script_runs_event_check CHECK (
        btrim(event) <> '' AND char_length(event) <= 100
    ),
    CONSTRAINT script_runs_outcome_check CHECK (
        outcome IN ('allow', 'deny', 'error', 'timeout', 'tripped')
    ),
    CONSTRAINT script_runs_duration_check CHECK (duration_ms >= 0),
    CONSTRAINT script_runs_error_check CHECK (
        error IS NULL OR char_length(error) <= 2000
    )
);

-- list_recent / retention purge order or range-scan by creation time.
CREATE INDEX script_runs_recent
    ON script_runs (created_at DESC, id DESC);

-- The circuit breaker reads the most recent outcomes per script.
CREATE INDEX script_runs_script_recent
    ON script_runs (script_id, created_at DESC, id DESC);
