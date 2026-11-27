#!/usr/bin/env python3
"""Measure HTTP streaming and interactive probes during real concurrent ingestion.

Run only inside perf_representative.py with a prebuilt release server. The media
parent must be disk-backed and already exist. All database, media and child
process mutations are restricted to this invocation's owned fixture and tree.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import http.client
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import uuid

from perf_ingestion_workload import FAILURE, SUCCESS, generate_corpus, query
from perf_ingress_workload import (ROOT, SOURCE, authenticate, digest, host_info,
                                   observed_request, percentiles, server_resources,
                                   slow_stream, start_server, stop_server,
                                   temporary_storage_info)
from run_perf_mutation_benchmarks import validate_fixture

MAX_SAMPLES = 5000


def validate_options(albums: int, timeout: int, media_seconds: int) -> None:
    if not 1 <= albums <= 200 or not 10 <= timeout <= 900 or not 30 <= media_seconds <= 1800:
        raise ValueError("albums must be 1..200, timeout 10..900, media seconds 30..1800")


def require_disk(path: Path) -> dict:
    info = temporary_storage_info(path)
    if info['filesystem'] in (None, 'tmpfs', 'ramfs', 'devtmpfs', 'overlay'):
        raise ValueError('media parent must be on a known disk-backed filesystem')
    return info


def stream_sample(port: int, token: str, public_id: str) -> dict:
    begin = time.monotonic()
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=45)
    try:
        connection.request('GET', f"/Audio/{public_id.replace('-', '')}/stream",
                           headers={'X-Emby-Token': token, 'Connection': 'close'})
        response = connection.getresponse()
        first = response.read(1)
        ttfb = time.monotonic() - begin
        body = response.read(65535)
        count = len(first) + len(body)
        if response.status != 200 or count != 65536:
            return {'status': response.status, 'ttfb_seconds': round(ttfb, 6),
                    'bytes': count, 'error': 'unexpected response'}
        return {'status': response.status, 'ttfb_seconds': round(ttfb, 6),
                'bytes': count, 'error': None}
    except (OSError, RuntimeError, ValueError, http.client.HTTPException) as error:
        return {'status': None, 'ttfb_seconds': None, 'bytes': 0, 'error': type(error).__name__}
    finally:
        connection.close()


def sample_loop(kind: str, port: int, token: str, public_id: str, stop: threading.Event,
                begin: float) -> list[dict]:
    samples = []
    while not stop.is_set() and len(samples) < MAX_SAMPLES:
        started = time.monotonic()
        sample = (stream_sample(port, token, public_id) if kind == 'stream' else
                  observed_request(port, 'GET', '/System/Info', token))
        sample['started_seconds'] = round(started - begin, 6)
        sample['finished_seconds'] = round(time.monotonic() - begin, 6)
        samples.append(sample)
        stop.wait(1.0 if kind == 'stream' else 0.25)
    return samples


def active_samples(samples: list[dict], first_active: float | None, last_active: float | None) -> list[dict]:
    if first_active is None or last_active is None:
        return []
    # Only requests wholly contained in the observed active window count.
    return [s for s in samples if s['started_seconds'] >= first_active and s['finished_seconds'] <= last_active]


def run(env: dict[str, str], binary: Path, parent: Path, albums: int, timeout: int,
        media_seconds: int) -> int:
    validate_fixture(env)
    validate_options(albums, timeout, media_seconds)
    binary = binary.resolve(strict=True)
    parent = parent.resolve(strict=True)
    if not binary.is_file() or not parent.is_dir():
        raise ValueError('server file and media parent directory must exist')
    storage = require_disk(parent)
    url = env['MELODEE__DATABASE__URL']
    initial = query(url, "SELECT json_build_object('database',current_database(),'work_items',(SELECT count(*) FROM release_work_items),'artists',(SELECT count(*) FROM artists),'releases',(SELECT count(*) FROM releases),'tracks',(SELECT count(*) FROM tracks),'postgres',version(),'shared_buffers',current_setting('shared_buffers'),'work_mem',current_setting('work_mem'),'max_connections',current_setting('max_connections'))")
    if initial['database'] != env['MELODEE_PERF_OWNED_DATABASE'] or initial['work_items'] != 0:
        raise ValueError('fixture database must be owned and contain no work items')
    initial.pop('database')
    with tempfile.TemporaryDirectory(prefix='melodee-perf-mixed-', dir=parent) as temporary:
        root = Path(temporary)
        for name in ('inbound', 'staging', 'work', 'storage', 'assets', 'batch'):
            (root / name).mkdir()
        source = root / 'inbound' / SOURCE
        source.mkdir()
        subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                        f'sine=frequency=437:duration={media_seconds}', '-metadata', 'album=Ingress Benchmark',
                        '-metadata', 'album_artist=Ingress Benchmark Artist', '-metadata', 'artist=Ingress Benchmark Artist',
                        '-metadata', 'title=Ingress Benchmark Track', '-metadata', 'track=1',
                        '-c:a', 'libmp3lame', '-b:a', '192k', str(source / '01.mp3')],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=240)
        media_bytes = (source / '01.mp3').stat().st_size
        (source / '.melodee-ready').touch()
        batch_bytes = generate_corpus(root / 'batch', albums)
        query(url, "WITH inserted AS (INSERT INTO native_user_roles (user_id,role,created_at) SELECT id,'HasStreamRole',now() FROM native_users WHERE normalized_username='BENCHMARK-1' ON CONFLICT DO NOTHING RETURNING 1) SELECT json_build_object('inserted',count(*)) FROM inserted")
        process, log, port = start_server(binary, root, url, 0, True)
        try:
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError('server exited during fixture ingestion')
                item = query(url, "SELECT COALESCE((SELECT json_build_object('state',w.state,'public_id',t.public_api_id) FROM release_work_items w LEFT JOIN catalog_imports c ON c.work_item_id=w.id LEFT JOIN tracks t ON t.release_id=c.release_id WHERE w.source_path='ingress-benchmark-release' ORDER BY t.ordinal LIMIT 1), 'null'::json)")
                if item and item.get('state') in SUCCESS and item.get('public_id'):
                    break
                if item and item.get('state') in FAILURE:
                    raise RuntimeError('fixture ingestion failed')
                time.sleep(0.5)
            else:
                raise RuntimeError('fixture ingestion timed out')
            public_id = str(uuid.UUID(item['public_id']))
            token = authenticate(port)
            stop, started = threading.Event(), threading.Event()
            begin = time.monotonic()
            with ThreadPoolExecutor(max_workers=3) as executor:
                slow = executor.submit(slow_stream, port, token, public_id, started, stop)
                try:
                    if not started.wait(45) or slow.done():
                        raise RuntimeError('slow stream failed to start')
                    # Each directory was fully written in the private batch area.
                    for directory in sorted((root / 'batch').iterdir()):
                        directory.rename(root / 'inbound' / directory.name)
                    batch_begin = time.monotonic()
                    stream = executor.submit(sample_loop, 'stream', port, token, public_id, stop, batch_begin)
                    probe = executor.submit(sample_loop, 'probe', port, token, public_id, stop, batch_begin)
                    first_active = last_active = None
                    states = {}
                    timed_out = False
                    while True:
                        if process.poll() is not None or slow.done():
                            raise RuntimeError('server or slow stream exited during ingestion')
                        states = query(url, "SELECT COALESCE(json_object_agg(state,n),'{}'::json) FROM (SELECT state,count(*) n FROM release_work_items WHERE source_path LIKE 'bench-release-%' GROUP BY state) s")
                        now = time.monotonic() - batch_begin
                        total = sum(states.values())
                        done = sum(states.get(state, 0) for state in SUCCESS)
                        if any(states.get(state, 0) for state in FAILURE):
                            break
                        if total > done:
                            first_active = now if first_active is None else first_active
                            last_active = now
                        if done == albums:
                            break
                        if now >= timeout:
                            timed_out = True
                            break
                        time.sleep(0.5)
                    elapsed = time.monotonic() - batch_begin
                finally:
                    stop.set()
                slow_result = slow.result(timeout=50)
                stream_samples = stream.result(timeout=50)
                probe_samples = probe.result(timeout=50)
            active_stream = active_samples(stream_samples, first_active, last_active)
            active_probe = active_samples(probe_samples, first_active, last_active)
            failures = [s for s in stream_samples + probe_samples if s['error'] is not None or s['status'] != 200]
            success = (not failures and not timed_out and done == albums and bool(active_stream)
                       and bool(active_probe) and len(stream_samples) < MAX_SAMPLES
                       and len(probe_samples) < MAX_SAMPLES)
            git = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
            dirty = bool(subprocess.run(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip())
            result = {'timestamp_utc': datetime.now(timezone.utc).isoformat(), 'repository_head_observed_at_end': git,
                      'tracked_worktree_dirty_at_end': dirty, 'binary_sha256': digest(binary),
                      'binary_source_identity': 'not independently established', 'host': host_info(),
                      'database_before_ingestion': initial, 'storage': storage, 'albums': albums,
                      'tracks_per_album': 1, 'batch_media': 'alternating 5-second generated MP3 and FLAC tones',
                      'batch_bytes': batch_bytes, 'stream_source_seconds': media_seconds,
                      'stream_source_bytes': media_bytes, 'stage_concurrency': 2,
                      'elapsed_seconds': round(elapsed, 6), 'states': states, 'timed_out': timed_out,
                      'active_observed_window_seconds': [first_active, last_active],
                      'active_stream_samples': active_stream, 'active_probe_samples': active_probe,
                      'all_stream_samples': stream_samples, 'all_probe_samples': probe_samples,
                      'stream_ttfb_percentiles_seconds': percentiles([s['ttfb_seconds'] for s in active_stream]) if active_stream and all(s['error'] is None for s in active_stream) else None,
                      'probe_latency_percentiles_seconds': percentiles([s['seconds'] for s in active_probe]) if active_probe and all(s['error'] is None for s in active_probe) else None,
                      'slow_stream': slow_result, 'request_failures': len(failures),
                      'server_resources': server_resources(process.pid), 'success': success,
                      'limitations': ['one host, loopback HTTP/1, synthetic audio and one Track per Release',
                                      'search, policy, discovery, enrichment and podcast workers disabled',
                                      'one slow stream and sequential repeated samples, no real player or audible underrun detection',
                                      'filesystem cache uncontrolled; no baseline comparison',
                                      'active window is bounded by polls with unfinished items and may include quiet gaps',
                                      'server resource samples exclude PostgreSQL and child processes']}
            print(json.dumps(result, sort_keys=True))
        finally:
            stop_server(process, log)
    return 0 if success else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--albums', type=int, default=200)
    parser.add_argument('--timeout', type=int, default=900)
    parser.add_argument('--media-seconds', type=int, default=1200)
    parser.add_argument('--server', type=Path, default=ROOT / 'target/release/melodee-server')
    parser.add_argument('--media-parent', type=Path, default=ROOT)
    args = parser.parse_args()
    try:
        return run(os.environ.copy(), args.server, args.media_parent, args.albums, args.timeout, args.media_seconds)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f'Mixed benchmark failed: {type(error).__name__}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
