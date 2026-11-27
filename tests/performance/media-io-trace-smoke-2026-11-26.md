# Media syscall profiler smoke, 2026-11-26

The optional Linux profiler exercised production inspection plus normalization
of one generated five-second file per case. It built and ran the release test
executable, then deleted the raw syscall traces and owned media directory.
This is a harness smoke and operation sample, not an end-to-end import or a
before/after result. Sanitized full records are [MP3](media-io-trace-mp3-smoke-2-2026-11-26.json)
and [FLAC](media-io-trace-flac-smoke-2026-11-26.json). An earlier MP3 diagnostic
omitted kernel-copy syscalls and is not comparable.

| Input | Input bytes | Attributed source reads | Attributed kernel-copy source reads | Attributed work reads | Attributed work writes | Attributed kernel-copy work writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| MP3 | 21,044 | 237,595 | 21,044 | 42,088 | 0 | 21,044 |
| FLAC | 93,228 | 556,252 | 0 | 43,624 | 21,812 | 0 |

Each case observed three ffprobe data-process executions (one inspection and
two normalization), one ffmpeg execution, and one fpcalc execution. The MP3
case retained the input; the FLAC case encoded MP3. Both used executable
SHA-256 `807bb86e8ecbd6b9a0a21b9f17dd75c930e5dbe55006d0e78d14787590869e63`
and profiler SHA-256
`54694bc0f81935d7091429673926bc53ed043c6ae7b85fb720e103f2418c39b0`.
The records include sampled source fingerprints and tool versions; these do
not independently prove that the executable was built from those sources.

The numbers sum successful logical syscall return bytes, not physical disk
traffic, distinct data, full-file hashes, or bytes required by a particular
stage. Kernel-copy counters are kept separate from userspace read/write calls
to avoid double counting the same syscall. The traces recorded 11 unparsed
file calls in each case. Process attribution starts at an observed `execve`, so
threads can fall under the generic process bucket; mmap page faults and
untraced syscall variants are not byte-attributed. Ptrace adds substantial
overhead, cache state was uncontrolled, and this sample does not establish a
performance improvement. Both used bare executable defaults; the existing
version-cache path resolution bug meant they did not run tool-version
subprocesses and lacked those manifest provenance values.

After building `perf_normalization_matrix` in release mode, run the profiler
with a new output path, for example:

```console
python3 scripts/perf_media_io_trace.py --binary target/release/deps/perf_normalization_matrix-EXACT_HASH --codec mp3 --seconds 5 --output tests/performance/media-io-trace-new.json
```

Linux `strace`, pidfd support and permitted tracing of an owned child are
required; no security configuration changes are made. The executable filename
must be replaced with the one reported by Cargo. The profiler checks its child
session, cancels descendants on timeout, limits trace size, and retains no raw
traces, paths, or credentials. Unexpected ptrace restrictions are a profiler
limitation, not evidence of an application behavior.
