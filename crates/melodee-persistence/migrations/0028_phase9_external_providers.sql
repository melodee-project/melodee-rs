-- external metadata providers and Last.fm account links
-- (ADR-0134 §7 as amended 2026-08-14, PRD-EXT-001..004, PRD-AUTH-002).
--
-- Additive. A binary downgrade leaves both tables intact; schema rollback
-- restores a verified pre-migration backup into a new database.
--
-- Nothing is seeded: provider cache entries are populated by provider calls
-- and Last.fm links exist only after a user completes the web-auth session
-- exchange (legacy ADR-0004). Provider payloads are cache/evidence only and
-- never become catalog identity (PRD-ARTIST-012, PRD-EXT-002).

-- Response cache/provenance home for external metadata providers. The
-- provider vocabulary is the enumerated legacy inventory (evidence:
-- legacy C# behavior audit); "AMG" named in
-- PRD-EXT-001 prose has no legacy implementation and is deliberately absent.
CREATE TABLE external_provider_cache (
    -- Provider identifier from the enumerated set below.
    provider text NOT NULL,
    -- Hashed lookup key (never the raw request, which can embed artist
    -- names/tokens); bounded for bounded index growth.
    request_key text NOT NULL,
    -- Cached provider payload; evidence trail for provenance
    -- (PRD-EXT-004). Bounded at 256 KiB serialized.
    payload jsonb NOT NULL,
    fetched_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,

    PRIMARY KEY (provider, request_key),

    CONSTRAINT external_provider_cache_provider_check CHECK (
        provider IN (
            'brave', 'deezer', 'discogs', 'itunes', 'lastfm',
            'metalapi', 'musicbrainz', 'spotify', 'wikidata'
        )
    ),
    CONSTRAINT external_provider_cache_request_key_check CHECK (
        btrim(request_key) <> '' AND char_length(request_key) <= 128
    ),
    CONSTRAINT external_provider_cache_payload_check CHECK (
        octet_length(payload::text) <= 262144
    ),
    CONSTRAINT external_provider_cache_expiry_check CHECK (
        expires_at > fetched_at
    )
);

-- Retention purge range-scan (ProviderCacheStore::purge_expired).
CREATE INDEX external_provider_cache_expiry
    ON external_provider_cache (expires_at);

-- Per-user Last.fm session links (legacy ADR-0004 lifecycle: web-auth
-- session exchange, no refresh flow, invalid-session clears the link).
-- The raw Last.fm session key is NEVER at rest in plaintext (deliberate
-- improvement over legacy's plaintext varchar(64) User.LastFmSessionKey;
-- evidence doc §3 item 3, §10.3): the table holds a SHA-256 digest for
-- lookup/audit and the AEAD-sealed key (ChaCha20-Poly1305 under a
-- server-held key derived from the security.secretKey setting) for runtime
-- api_sig signing. Neither column is ever exposed by an API.
CREATE TABLE native_lastfm_links (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE CASCADE,
    -- Last.fm username from the session exchange; NULL because the legacy
    -- flow discards it (evidence doc §3) and some responses omit it.
    lastfm_username text NULL,
    session_key_digest bytea NOT NULL,
    session_key_ciphertext bytea NOT NULL,
    linked_at timestamptz NOT NULL,

    CONSTRAINT native_lastfm_links_username_check CHECK (
        lastfm_username IS NULL OR (
            btrim(lastfm_username) <> '' AND char_length(lastfm_username) <= 64
        )
    ),
    CONSTRAINT native_lastfm_links_digest_check CHECK (
        octet_length(session_key_digest) = 32
    ),
    -- 12-byte random nonce + 16-byte Poly1305 tag + the short session key;
    -- the generous ceiling bounds row size without admitting junk.
    CONSTRAINT native_lastfm_links_ciphertext_check CHECK (
        octet_length(session_key_ciphertext) BETWEEN 29 AND 1024
    )
);
