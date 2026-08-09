# Phase 0 Evidence — Media-Processing Behavioral Baseline

**Issue:** FIRST-10-ISSUES.md Issue 8 — Media-processing behavioral baseline
**Requirements:** PRD-MEDIA-001..003, PRD-INGEST-001..005, PRD-STORAGE-001..004
**Baseline:** legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (authoritative SHA);
server version `2.2.0`; DB migration head `20260714155936_AddDeleteSourceResidueAfterIngestSetting`
**Recorded:** 2026-08-08
**Artifacts:** `compatibility/media/baseline.json` + `compatibility/media/cases/*.json` (31
cases). Generator: `scripts/generate_media_corpus.py` (deterministic, ffmpeg-synthesized, fully
redistributable — no copyrighted media). Harness: `scripts/capture_media_baseline.py`.

> **Scope note on "current behavior":** this document describes *measured legacy behavior*
> (drive: `mcli`, the same `Melodee.Common` code as the server, path-based inbound→staging on
> the disposable DB). Where the **new Rust product requirement** intentionally differs, it is
> marked **[RUST]** — nothing here silently blesses legacy behavior.

## 1. Method and environment caveats

- Pipeline drive: `mcli tags <file> show` (production `AtlMetaTag` + `MetaTagsProcessor`),
  `mcli library process --inbound X --staging Y` (production
  `DirectoryProcessorToStagingService`), `mcli validate album --file` (production
  `AlbumValidator`). No Quartz, no server required.
- **Environment-conditional:** the fixture artist search engine is EMPTY, so
  `ArtistHasSearchEngineResult` fails every album and `AlbumStatus.Ok` is unreachable here;
  every staged album is `Invalid` with `HasInvalidArtists`. All other validation rules are
  exercised normally and their messages/reasons are recorded per case. External providers
  (MusicBrainz/Last.fm/Spotify) are offline.
- `mcli tags show` always exits 1 (`isValid` never set — recorded CLI defect).

## 2. Supported formats and the metadata-container gate

Accepted extensions (FileHelper.cs:26-48): `aac, alac, ac3, bonk, aiff, ape, flac, m4a, m4b,
mp1, mp2, mp3, oga, ogg, opus, spx, sfu, tta, wav, wma`. Content type: magic bytes first
(`FF FB`, `fLaC`, `RIFF`, `OggS`, `ftyp`), else extension lookup.

**CRITICAL BASELINE FINDING (F1): the ingest gate requires a recognized METADATA container.**
`AtlMetaTag` rejects any file whose ATL `MetadataFormats` contain ID < 0
(AtlMetaTag.cs:191). Measured consequences:

| Case | Outcome |
|---|---|
| Untagged PCM WAV (clean-wav, multichannel) | **0 albums; zero technical fields extracted; sources untouched; no conversion attempted** |
| WAV content in `.mp3` name (not-mp3-named-mp3) | **0 albums; run exits 1** |
| Bare MP3 stream without ID3 (missing-tags) | Extraction all-null; pipeline falls back to directory/filename parsing and produces an album with filename-derived titles but no artist resolution |
| CUE source WAV | The `.cue` sidecar is what makes the album: ffmpeg split to MP3s (which carry ID3) — the album exists only via the sidecar path |

So despite `wav` being in the accepted extension list and WAVs streaming fine from the DB
(Issue 5-7 evidence), **a real library scan never ingests tagless WAV/AIFF files at this
baseline** — no error surfaced to users beyond debug logs. [RUST: PRD-MEDIA requires media
inspection independent of metadata presence; do NOT preserve the silent skip.]

## 3. Metadata extraction (per-format evidence in `cases/*/extraction`)

- Reader: **ATL** (`AtlMetaTag`, SortOrder 0); fallback `NativeId3MetaTag` (MP3 only).
- Full tag list with ID3-frame mapping per MetaTagIdentifier enum (research-confirmed).
  Measured per format: MP3/FLAC/OGG/OPUS/M4A all yield Title/Album/AlbumArtist/Album/
  RecordingYear/Genre/TrackNumber/DiscNumber(+forced 1)/Bpm(0)/OrigAlbumYear/AlbumDate/Date/
  Encoder; technical MediaAudios (BitRate, SampleRate, Channels, CodecLongName, Duration,
  IsVbr) populated except BitDepth (-1 except FLAC 16).
- **Artist tag nullification:** when per-song `Artist` == `AlbumArtist`, the Artist value is
  **nullified** by the Artist metaTag processor (Artist.cs:113-126, verbatim comment: "If the
  value for the 'Artist' matches the 'AlbumArtist' then nullify"). Measured on all clean cases.
- Synthesized fields: `OrigAlbumYear` forced (0 if absent), `AlbumDate`/`Date` derived from
  year, `DiscTotal` **always forced to 1** (DiscTotal.cs processor), `AlbumArtist` synthesized
  from Artist when missing.
- Multi-value: `;`/`;;` normalized to `/`; genres split on `/` at insert.
- ReplayGain tags are read but **not persisted** to DB song rows (LibraryInsertJob.cs:561-589).
- `Duration` is an int (seconds, rounded — 0.5s blip → 1).

## 4. Conversion behavior (measured)

Gate: `conversion.enabled=true` (seed) AND magic-byte detection says known-non-MP3 audio
(`FileFormatDetector`; an `.mp3`-named file failing MP3 detection also converts).

| Aspect | Measured behavior |
|---|---|
| Command | FFMpegCore `libmp3lame`, `WithAudioBitrate(conversion.bitrate=384)`, `WithAudioSamplingRate(conversion.samplingRate=48000)`, `WithVariableBitrate(conversion.vbrLevel=4)`, force `mp3` — **no channel mapping** (channels preserved) |
| Output | `<same dir>/<base>.mp3`; staged as `NNNN Title.mp3` (4-digit padding) |
| Source treatment | `processing.doDeleteOriginal=false` (seed): source renamed `<name>._converted`; converted MP3 ALSO left in inbound (copy mode) |
| 96 kHz FLAC | Converted; output at 48000 Hz (measured in staged file set) |
| 5.1 WAV | Not reached (WAV gate, F1) |
| Failure | ffmpeg exception: partial output kept if usable MP3, else recorded; run continues (per-directory error tolerance; mcli exits 1 with output present) |

**Seeded streaming transcoding templates** (`transcoding.command.mp3/opus/aac`,
`transcoding.default=raw`) are **malformed JSON** (unterminated `'bitrate` string) and have
**no consumer** at this commit — streaming transcoding does not exist (confirmed Issues 6/7:
OS ignores maxBitRate; Jellyfin rejects transcode requests). Ingest conversion (`conversion.*`)
is the only real ffmpeg path. **[RUST]** the desired canonical-storage/VBR policy in the PRD
is a *new* product requirement — the legacy system converts non-MP3 → MP3 384k VBR-q4 48kHz
on ingest and does nothing else.

## 5. Naming, artwork, and filesystem layout

- Staging dir: `<ArtistSort> - [<Year>] <AlbumTitle>/`; storage: `<ArtistDirectory>/[<YYYY>] <Title>/`.
- Songs: `NNNN Title.mp3` (4-pad). **Filename sanitization strips non-ASCII**: `Neon 東京` →
  `0001 Neon.mp3`; `Eple (Røyskatt Mix)` → `0002 Eple (Ryskatt Mix).mp3`; `Ünïcode Söng` →
  `0003 ncode Sng.mp3` (measured; album title keeps Unicode in metadata/displaySummary).
- Images: embedded → extracted as `i-NN-<PicType>.jpg` (deduped CRC32, wider wins), external
  cover classified by filename keywords (Front/SecondaryFront/Artist...); SecondaryFront
  promoted to Front when no Front. 4000px cover → resized to the **1600px square tier**
  (tiers 300/600/1600). Invalid artwork (garbage bytes) → **deleted**, album flagged
  HasNoImages. Renumbered to `i-01-Front.jpg` etc. Album with no images → album-image search
  engine download attempt (offline here → HasNoImages stands).
- Measured quirk: in `embedded-artwork`, the source song that donated artwork kept its
  original filename while siblings were renumbered (recorded as observed).

## 6. Validation model (AlbumValidator.ValidateAlbum)

Status = Ok iff no Critical messages, else Invalid; `AlbumNeedsAttentionReasons` flags.
Seeded thresholds: `validation.minimumSongCount=3`, `validation.minimumAlbumDuration=10`
minutes, `validation.maximumSongNumber=9999`, years 1860..2150. Measured per case (see
`cases/*/pipeline.validations`):

- Sequential-numbering rule is strict: numbers must start at 1 and be gap-free
  (`SongsAreNotSequentiallyNumbered`, `HasInvalidFirstSongNumber` on multi-disc 1,2,1,2 and
  cue-split).
- Missing TRCK → SongNumber 0 → `0000 Unknown Position.mp3`, invalid song.
- **Duplicate track numbers were NOT flagged** in duplicate-track-numbers (two TRCK=1 songs
  remain 1,1,2; magic renumbering not observed) — recorded; the `AreSongsUniquelyNumbered`
  rule did not fire as expected. Marked as a behavior to re-verify against richer fixtures in
  Phase 2.
- Undesired-severity flags (not invalidating): `TitleHasUnwantedText` on "(feat. ...)" album
  title.
- Validation categorization for the Rust rewrite: true structural (song numbering, durations),
  metadata quality (year, artist), **enrichment dependency** (`ArtistHasSearchEngineResult` —
  external service, NOT structural), artwork (HasNoImages), policy (`IsNotStudioTypeAlbum`).
  [RUST: PRD-INGEST-002 separates structural validity from enrichment/policy.]

## 7. Classification and destructive-policy hazards

- Title-based type: EP/Single/live/tribute regexes (StringExtensions.cs:733-756). Measured:
  "Signals Live at the Depot" → type Other + `IsNotStudioTypeAlbum` (needs manual validation).
  "Signals EP" → title trimmed to "Signals" but type stayed **Album** (recorded as observed —
  the EP regex strips but the studio-type rule outcome differs from naive expectation).
- Directory-based: `IsDirectoryNotStudioAlbumsRegex`
  `(single|best of|greatest hit(s)|compilation|live|boxset|bootleg|promo|demo)` → Invalid +
  stays in staging. **Not destructive** (no delete/quarantine on these keywords at this
  commit) — but they silently block auto-storage, which is itself a data-availability hazard.
- **No keyword-based deletion exists.** Destructive actions at baseline are limited to:
  duplicate song/image deletion, invalid-image deletion, sidecar deletion (move mode),
  residue cleanup (`processing.deleteSourceResidueAfterIngest=true`), empty-dir removal.
  `[RUST]` any low-confidence destructive rule would be a migration hazard and is absent here.

## 8. Hashing and duplicates

- `FileHash` = CRC32 (`X8` uppercase) over the whole file (Utility/CRC32.cs) — NOT a
  cryptographic hash. `duplicateHashCheck` = custom 32-bit mix of (album, SongNumber,
  normalized title).
- Album-level duplicates on insert: skipped or directory renamed with
  `processing.duplicateAlbumPrefix = "_duplicate_ "` (trailing space; storage move skips
  those dirs).
- **DESTRUCTIVE BUG (F2):** `FindDuplicatesAsync`
  (FileSystemDirectoryInfoExtensions.cs:28-89) deletes **every same-size file after the
  first, regardless of SHA-256 equality** — the grouping filter keeps all non-"pending"
  entries in any size-collision group. Deterministically reproduced (`same-size-files`): two
  DIFFERENT songs at exactly 49721 bytes → song 1 deleted from staging, while `melodee.json`
  **still references the deleted file**. [RUST: never delete on size collision; compare
  content (and only then quarantine, never delete — AGENTS.md data-safety rules).]
- Missing-file-mid-process and unstable-file behavior recorded from code with CLI evidence
  where reproducible: per-file stability waits; corrupt MP3 (4 KiB truncation) → song
  skipped, album continues with 2 songs and HasLessThanMinimumSongs.

## 9. Sidecars

- `.cue` → parsed (Windows-1252 fallback), ffmpeg split with conversion settings, per-song
  tags; disc fields absent; third song sortOrder 0 (recorded quirk).
- `.sfv`/`.m3u`/`.nfo` → parsed by directory plugins; sidecar retained (copy mode); residue
  deletion only in move mode (`processing.doDeleteOriginal=false` here).
- `.lrc` → present but lyrics plugin requires `lyrics.filesEnabled`; not observed in staged
  song tags (recorded).
- `.mtg` Melodee tag files applied then deleted (not exercised in this corpus).

## 10. Structural-validation reason categories (for PRD-INGEST-002)

| Observed reason | Category |
|---|---|
| HasNoSongs, HasInvalidSongs, SongsAreNotSequentiallyNumbered, HasInvalidFirstSongNumber, SongTotal mismatch | structural media problem |
| HasInvalidYear, HasNoTags, HasLessThanMinimumSongs/Duration | metadata quality/policy thresholds |
| HasInvalidArtists, ArtistHasSearchEngineResult | **enrichment dependency (external)** — must not gate structural validity in Rust |
| HasNoImages | artwork |
| IsNotStudioTypeAlbum, TitleHasUnwantedText | release classification/policy |

## 11. Failure and interruption behavior (measured + code-verified)

- Corrupt/truncated audio → song skipped, album continues.
- Invalid artwork → deleted, run exits 1 with album still produced (per-directory error
  tolerance; `processing.doContinueOnDirectoryProcessingErrors=true`).
- Conversion failure → partial kept if usable MP3, else recorded; run continues.
- Unstable (being-written) files → directory deferred (double-snapshot check).
- Missing song file referenced by melodee.json at insert → album songs cleared, json deleted,
  reprocess event (LibraryInsertJob.cs:539-556, code-verified).
- **No atomicity guarantees**: partial mutations persist (matching the API-layer findings
  cluster). Interruption/restart semantics are not transactional anywhere in the pipeline —
  the primary motivation for Phase 1A durable work items.

## 12. Key legacy deficiencies (input to Rust requirements, NOT things to preserve)

1. **F1** — metadata-container gate silently skips tagless WAV/AIFF and bare streams.
2. **F2** — same-size file deletion without content comparison (plus dangling melodee.json
   references).
3. Ingest converts everything non-MP3 to MP3 384k VBR-q4 48kHz; no lossless-canonical
   option exists; streaming transcoding is advertised-but-absent.
4. Artist==AlbumArtist nullification loses per-song artist fidelity.
5. DiscTotal forced to 1; multi-disc only survives via MediaNumber/sortOrder convention;
   sequential-numbering validation flags real multi-disc releases.
6. Filename sanitization silently strips non-ASCII from staged filenames.
7. Validation conflates structural problems with enrichment availability and policy.
8. `mcli tags show` exit-code defect; `validate album --json` output is line-wrapped into
   invalid JSON (harness flattens before parsing).

## 13. Privacy/redistribution review

Every media artifact is synthesized by the committed generator from sine waves and flat-color
images. No copyrighted content, no external downloads, no credentials anywhere in
`compatibility/media/`.
