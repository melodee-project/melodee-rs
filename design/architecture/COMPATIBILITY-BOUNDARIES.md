# Compatibility Boundaries

## Native Melodee API v1

First-party performant client contract. Preserve existing DTOs, pagination, errors, JWT/refresh
behavior, public IDs, image/stream URLs, and semantics used by Android/Android Auto and MeloAmp.

## OpenSubsonic

Ecosystem compatibility contract. Preserve form/query auth, JSON/XML/JSONP envelopes, plain and
`.view` forms, advertised extensions, direct/transcoded streaming, and intentional 410/501 behavior.

## Jellyfin

Music-only compatibility contract. Preserve discovery, login/token forms, item/artist/genre/audio/
image/playlist/session behavior implemented today. Do not imply full Jellyfin parity.

## Admin API

New Rust-native contract for processing, evidence, identities, policies, workers, storage, and
curation. It must not force changes into native v1.

## Rule

Protocol DTOs never become domain entities. Each adapter maps to application queries/commands and owns
its exact wire behavior.
