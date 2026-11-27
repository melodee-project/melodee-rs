# Shared decode investigation (2026-11-26)

This is a test-only investigation, not production ingestion or a completed RM-11
optimization. The experiment uses locally generated synthetic signals and owned
temporary directories, removed after execution. It compares a full FFmpeg
`-xerror` decode plus `fpcalc -json -length 120` with a single FFmpeg decode
split into full validation and a Chromaprint-muxer branch. FFmpeg and ffprobe
were 9.0.2; fpcalc was 1.6.1 using Lavc63.1.101, Lavf63.1.101, SwR7.1.101.
Exact arguments, timestamps, script hashes, timings, and outcomes are retained
in the JSON files. The fingerprint algorithm uses `-algorithm 1` for FFmpeg's
zero-based muxer option, corresponding to fpcalc's default algorithm.

The original `atrim=duration=120` before resampling matched each short MP3,
FLAC, AAC and Opus fingerprint in two repetitions, but mismatched the 125 s
MP3 in both repetitions. Its optional strace samples reported about half as
many attributed logical source-read bytes for the candidate as the two
separate processes; those traced, synthetic numbers are not physical I/O or a
production workload measurement. See
[extended results](chromaprint-shared-decode-extended-2026-11-26.json).

The revised branch resampled to 11,025 Hz, mono, signed 16-bit audio before
trimming at 1,323,000 samples. Its five synthetic cases matched fingerprint
output in 10/10 trials, including 125 s MP3; see
[resampling results](chromaprint-shared-decode-resample-2026-11-26.json).
A further branch also configured swresample with filter size 16, phase shift 8,
linear interpolation 1 and cutoff 0.8, as fpcalc's source does. It matched
20/20 trials across 10 generated cases: MP3, FLAC, AAC and Opus; 32–96 kHz;
and 119.9, 120, 120.1 and 125 s cutoff cases. In that run a truncated FLAC was
rejected by both commands and an intentionally timed-out child session was
terminated and reaped. See
[compatible-resampler results](chromaprint-shared-decode-compatible-2026-11-26.json).
These examples do not establish output equivalence over arbitrary inputs,
installed binary versions, stream layouts, errors, or cancellation paths.

The source explains the cutoff hypothesis: fpcalc configures output channels
and sample rate from Chromaprint, and applies its sample limit **after** the
reader's resampling ([fpcalc](https://raw.githubusercontent.com/acoustid/chromaprint/v1.6.1/src/cmd/fpcalc.cpp),
[reader](https://raw.githubusercontent.com/acoustid/chromaprint/v1.6.1/src/audio/ffmpeg_audio_reader.h),
[resampler settings](https://raw.githubusercontent.com/acoustid/chromaprint/v1.6.1/src/audio/ffmpeg_audio_processor_swresample.h)).
The local Chromaprint library reported mono and 11,025 Hz. This is not proof
that an independently installed fpcalc and FFmpeg muxer use the same library,
parameters, decoder, selected audio stream, or flush and padding semantics.

Duration remains a separate correctness issue. For a generated Opus input,
ffprobe reported 15.0065 s while fpcalc JSON reported 15.01 s; a second Opus
and a 96 kHz AAC input also differed. Upstream fpcalc uses its selected audio
stream's integer time base and duration to compute integer milliseconds,
falls back to container duration when unavailable, and prints seconds to two
decimal places. The experimental reconstruction matched the 20 generated
samples, but is not proven exact for absent/malformed duration, separate
library versions, multi-stream selection, rounding environments, or all
containers. It is not used in production. Independent ffprobe duration must
not silently replace fpcalc's duration.

The test supervisor exercises owned process cleanup, not production lease
fencing or publication. Existing independent validation, source/output
observations, custody hashes, and production fpcalc behavior remain intact.
Performance timings include process launch and a 50 ms polling interval, have
uncontrolled cache conditions, and do not establish application-level speedup.

A separate single-repetition run added a PCM output branch solely to count
actual samples; this changes the filter graph and timing. At 119.9 s the
branch emitted 1,321,898 mono samples, and at 120, 120.1 and 125 s it emitted
1,323,000. Ten generated fingerprints matched. For an owned Matroska file
with two audio streams and the second marked default, fpcalc and the candidate
mapping the first audio stream produced **different** fingerprints. A
conservative proposed fallback rejects multistream inputs before using a
candidate result; no such production guard or optimization has been added.
The synthetic metadata tests exercise a format-duration approximation and
missing/nonfinite duration, but do not establish actual format fallback or
unknown-duration equivalence. Full details and exact commands are in
[sample-count results](chromaprint-shared-decode-samples-2026-11-26.json).

A follow-up generated an ordinary Matroska file without audio-stream duration:
its container duration was 15.123 s and fpcalc returned 15.12 s, matching the
approximation for that case. A live Matroska file had neither duration and
fpcalc returned 0.0 s; the approximation returned unknown. The conservative
candidate must fall back for the latter. The multi-stream mismatch, truncated
FLAC rejection, and test-only timeout cleanup were reproduced. See
[metadata results](chromaprint-shared-decode-metadata-2026-11-26.json).
No real music, unusual channel layouts, inconsistent/broken timestamp or
corrupt container matrix, independently versioned executables, production
lease loss, or post-cancellation publication behavior was verified by these
experiments. They cannot establish a safe production substitution.
The existing `fingerprint_file` rejects fpcalc's zero duration, so fallback
must retain that error rather than accepting an invented duration. Configured
`fpcalc` and `ffmpeg` paths may be arbitrary executables and wrappers; their
output, error, timeout and provenance semantics cannot be inferred from the
presence of an FFmpeg muxer. The repository's existing wrapper tests cover
this supported configuration. These observations preclude an equivalent
production substitution on the present evidence.
