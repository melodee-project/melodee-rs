# Media custody operation inventory, 2026-11-26

This inventory is a source trace, not a measurement of physical disk I/O.
Counts are conditional on the path and exclude metadata lookups, filesystem
cache behavior, tool startup/version-cache misses, and extra work from retries.
The current implementation and the final benchmark must be checked again if
their source changes.

One focused operation-count regression exercised real retained-MP3
normalization through an ffprobe wrapper: after removing an unused
normalization-only version lookup it observed two independent data probes and
zero version probes. The removed call could spawn a version subprocess on a
cold cache when configured with a resolvable executable path. At the time of
the original trace, bare `ffprobe` defaults failed the version cache's metadata
lookup before spawning, so there was no subprocess to save in that case and
inspection's version provenance was absent. The later PATH-resolution fix
restores that provenance. A subsequent review found that a pathname can switch
from executable A to B and back during a version probe, so before/after
identity checks cannot safely bind the output to A. Persistent version-result
reuse is now disabled for unbound path execution. The bounded per-identity
in-flight gate remains, but waiting requests retry instead of sharing a result;
this serializes rather than eliminates concurrent probes. A deterministic ABA
regression and the related version tests passed. Bare names on Windows and Android
use native search without persistent caching. An individual uncached probe can
still observe whichever executable the operating system starts during an ABA
race. Binding execution to a verified, immutable image and restoring safe
caching remain open; independent source/output media probes are retained.
This is a small cold-path reduction, not a measurement of file I/O or
completion of RM-11. The 14 real-media normalization tests passed afterward.

| Stage and path | Cryptographic reads, copies, and temporary files | External work |
| --- | --- | --- |
| New same-filesystem acquisition | Rename without replacement; hash the staged tree for the receipt. Cleanup sees an absent inbound path after a successful rename. | None. |
| New cross-filesystem acquisition | Hash the source tree before copy; copy each file to a partial tree; hash the source and copied trees independently; sync and rename without replacement. Before deleting a still-matching inbound occurrence, hash it again. | None. |
| Staged recovery | Verify the durable receipt against the staged tree before adopting it. If an interrupted finalized tree has no completed receipt, recover and verify its relationship to the inbound occurrence before recording a receipt. Cleanup rechecks the inbound generation and tree. | None. |
| Inspection | After acquisition, verify the staged tree; inspect each audio file with a full-file hash and native-tag/artwork reads; verify the staged tree again after inspection. Snapshot and optional processing-gate work occur between these checks. | One ffprobe per audio file, plus cached tool-version lookup. |
| Normalization admission | Resolve the durable staging receipt and hash its tree; immediately hash the same bound tree again before loading the manifest and starting work. A full staged-tree verification also follows normalization. | None at these tree checks. |
| Retained MP3 | Hash source before processing; full decode check; copy source to a new output; independently hash output; fingerprint source; hash source afterward. An embedded-artwork read triggers a further source verification. | Source ffprobe, full-decode ffmpeg, output ffprobe, fpcalc; embedded artwork adds ffmpeg work. |
| Transcoded FLAC/other audio | Hash source before and after; encoder reads source and writes a new output; independently hash output; fingerprint source. Embedded-artwork extraction adds a further source verification. | Source ffprobe, ffmpeg encoder, output ffprobe, fpcalc; embedded artwork adds ffmpeg work. |
| CUE | Hash and probe the whole source; for each segment encode, hash, and probe its own new output; fingerprint whole source and hash it afterward. | One source ffprobe and fpcalc, plus ffmpeg and output ffprobe per segment. |
| External artwork | Bounded preflight reads and hashes the source bytes used for header/dimension checks, or hashes rejected oversized input; ffmpeg reads source and writes a temporary output; verification reads output once for size, hash, JPEG header, and dimensions; source is rehashed after processing. | One ffmpeg normalization per accepted source. |
| Validation | Independently read and hash each normalized output against its declared hash and size. | No audio subprocess. |
| Fresh local publication | Read each normalized asset while copying and hashing it, sync and install without replacement, then independently hash the installed incoming asset before `AssetCopied`. Verify all assets in the sealed incoming bundle; after rename and parent sync, verify the final canonical bundle again. | None. |
| Fresh NFS publication | Hash during copy into exclusive scratch and install; after sealing and parent sync, independently verify the final bundle. The existing unit test `fresh_nfs_charges_one_write_one_read_and_adoption_rehashes` accounts for one asset-sized write and read, then a further read for adoption. | None. |
| Interrupted publication/adoption | Existing partial or final destinations are independently verified; mismatched or foreign bytes are not overwritten. Fsync, no-replace installation and receipt verification still apply. | None. |

The apparent adjacent normalization checks use the same durable receipt and
have no source operation between them. They are presently separate observations:
a same-size change between them can produce the persisted
`normalization.staged_source_integrity` failure, whereas an earlier receipt
failure produces `normalization.staged_source_missing`. Collapsing them changes
that behavior and does not prove the tree was immutable. The later post-work
check is a separate boundary. No reduction is claimed here.

Likewise, local publication's `AssetCopied` is a per-asset milestone persisted
by the publication worker. It follows an independent installed-file check and
can trigger cancellation or concurrent mutation through the observer before
the manifest is written. Deferring that read to bundle sealing changes both
milestone timing and what progress persists on a subsequent failure. Recovery
and adoption re-read existing bytes. The redundant-looking checks therefore
have not been removed on the basis of ownership or metadata alone.

An experimental retained-MP3 candidate-copy design was rejected: replacing
the separate source and output probes with a single self-compared candidate
probe could accept transiently altered probe observations that the independent
comparison would reject. It was removed without benchmarking or a completion
claim. A safe alternative must bind the subprocess observation to verified
bytes while preserving source and output boundaries, cancellation, no-clobber
installation, retries, and platform behavior.
