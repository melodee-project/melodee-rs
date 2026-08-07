# Compatibility Test Strategy

## Layers

1. Route/schema inventory.
2. Golden request/response snapshots on controlled fixtures.
3. Differential C# versus Rust execution.
4. Stateful workflow tests (auth refresh, playlist changes, scrobble, queue, sessions).
5. Streaming/range/transcoding tests.
6. Real-client smoke/long-running tests.

## Comparison discipline

Compare wire behavior before mapping it into a common semantic representation. The purpose is to catch
what clients observe, including odd legacy behavior. Classify each difference as defect, intentional
legacy behavior, approved correction, unsupported fixture, or nondeterminism requiring an explicit
normalizer.

## Release gates

The Rust server is not a replacement candidate until Android/Android Auto, MeloAmp, representative
OpenSubsonic clients, and supported Jellyfin clients complete documented critical paths against a
migrated copy of the catalog and user state.
