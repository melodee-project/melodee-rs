#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import random
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate deterministic synthetic catalog JSONL")
    parser.add_argument("--artists", type=int, default=100)
    parser.add_argument("--releases-per-artist", type=int, default=2)
    parser.add_argument("--tracks-per-release", type=int, default=10)
    parser.add_argument("--seed", type=int, default=20260807)
    parser.add_argument("--output", type=Path, default=Path(".local/synthetic/catalog.jsonl"))
    args = parser.parse_args()

    if min(args.artists, args.releases_per_artist, args.tracks_per_release) < 1:
        raise SystemExit("all counts must be positive")

    random.seed(args.seed)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as stream:
        for artist_index in range(1, args.artists + 1):
            artist_id = f"artist-{artist_index:08d}"
            for release_index in range(1, args.releases_per_artist + 1):
                release_id = f"release-{artist_index:08d}-{release_index:04d}"
                year = 1950 + random.randrange(77)
                for track_index in range(1, args.tracks_per_release + 1):
                    value = {
                        "artistId": artist_id,
                        "artistName": f"Synthetic Artist {artist_index:08d}",
                        "releaseId": release_id,
                        "releaseTitle": f"Synthetic Release {release_index:04d}",
                        "year": year,
                        "trackId": f"{release_id}-track-{track_index:04d}",
                        "trackTitle": f"Synthetic Track {track_index:04d}",
                        "trackNumber": track_index,
                        "durationMs": random.randrange(30_000, 600_000),
                    }
                    stream.write(json.dumps(value, separators=(",", ":")) + "\n")

    total = args.artists * args.releases_per_artist * args.tracks_per_release
    print(f"Wrote {total:,} synthetic Tracks to {args.output}")


if __name__ == "__main__":
    main()
