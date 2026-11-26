-- PERF-012 / RM-06: durable discovery maintenance progress.
--
-- The previous pass reset every popularity cursor to Uuid::nil() and shared
-- one batch budget across three missing-candidate passes, three popularity
-- passes, and chart relinking. Under default configuration the track prefix
-- consumed the whole budget, so Releases, Artists, and chart relinking were
-- never reached and progress did not survive a restart.
--
-- This table persists one durable keyset cursor per work class plus an explicit
-- wrap marker. Work is advanced only after the corresponding durable effect
-- commits, so a crash replays an idempotent batch rather than skipping it.
-- Additive only; a binary downgrade ignores the table and the old code path
-- simply stops using it (the missing-candidate backstop remains correct).

CREATE TABLE native_discovery_progress (
    work_class text PRIMARY KEY,
    cursor_uuid uuid NULL,
    cursor_chart_id uuid NULL,
    cursor_rank integer NULL,
    pass_completed boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL,

    CONSTRAINT native_discovery_progress_class_check CHECK (
        work_class IN (
            'track_popularity',
            'release_popularity',
            'artist_popularity',
            'missing_candidates',
            'chart_relink'
        )
    ),
    CONSTRAINT native_discovery_progress_cursor_shape_check CHECK (
        (work_class = 'chart_relink'
            AND cursor_uuid IS NULL
            AND (cursor_chart_id IS NULL) = (cursor_rank IS NULL))
        OR (work_class <> 'chart_relink'
            AND cursor_chart_id IS NULL
            AND cursor_rank IS NULL)
    ),
    CONSTRAINT native_discovery_progress_rank_check CHECK (
        cursor_rank IS NULL OR cursor_rank >= 0
    )
);

INSERT INTO native_discovery_progress (
    work_class, cursor_uuid, cursor_chart_id, cursor_rank, pass_completed, updated_at
) VALUES
    ('track_popularity', NULL, NULL, NULL, false, now()),
    ('release_popularity', NULL, NULL, NULL, false, now()),
    ('artist_popularity', NULL, NULL, NULL, false, now()),
    ('missing_candidates', NULL, NULL, NULL, false, now()),
    ('chart_relink', NULL, NULL, NULL, false, now())
ON CONFLICT (work_class) DO NOTHING;
