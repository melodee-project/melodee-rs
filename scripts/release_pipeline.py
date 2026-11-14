#!/usr/bin/env python3
"""Validate, archive, and publish exactly qualified release images.

Network and mutation happen only after parsing a selected command. Publication
requires the release workflow's credentials and serialized GitHub environment;
--help never contacts a service. Exact-version tags are immutable.
"""
from __future__ import annotations
import argparse
import gzip
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
IMAGE = 'ghcr.io/melodee-project/melodee'
DIGEST = re.compile(r'sha256:[0-9a-f]{64}\Z')
SEMVER = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z.-]+))?\Z')


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def capture(command: list[str]) -> str:
    return run(command, capture_output=True, text=True).stdout.strip()


def output(**values: str) -> None:
    with Path(os.environ['GITHUB_OUTPUT']).open('a', encoding='utf-8') as stream:
        for key, value in values.items():
            if '\n' in value or '\r' in value:
                raise ValueError('invalid multiline workflow output')
            stream.write(f'{key}={value}\n')


def version_metadata(tag: str, prerelease: str, packages: list[dict]) -> dict[str, str]:
    version = tag.removeprefix('v')
    match = SEMVER.fullmatch(version)
    if not match or prerelease not in {'true', 'false'}:
        raise ValueError('release requires valid SemVer and a boolean prerelease flag')
    if match[4]:
        parts = match[4].split('.')
        if any(not part or (part.isdecimal() and len(part) > 1 and part[0] == '0') for part in parts):
            raise ValueError('invalid SemVer prerelease identifiers')
        if prerelease != 'true':
            raise ValueError('SemVer prerelease must be marked prerelease')
    versions = [p['version'] for p in packages if p['name'] in {'melodee-server', 'melodee-cli'}]
    if len(versions) != 2 or any(item != version for item in versions):
        raise ValueError('release tag must match both application package versions')
    return {'version': version, 'minor': f'{match[1]}.{match[2]}', 'stable': str(prerelease == 'false').lower()}


def revision_matches(expected: str) -> None:
    actual = capture(['git', 'rev-parse', 'HEAD'])
    if not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', expected) or actual != expected:
        raise ValueError('checkout differs from the validated release revision')


def evidence_dir() -> Path:
    sys.path.insert(0, str(ROOT / 'scripts'))
    import validate_release_evidence as evidence
    path = Path(os.environ['EVIDENCE_DIR'])
    if evidence.contains_symlink_component(path):
        raise ValueError('evidence directory must not traverse symlinks')
    return path


def evidence_module():
    sys.path.insert(0, str(ROOT / 'scripts'))
    import validate_release_evidence
    return validate_release_evidence


def record_image() -> None:
    path = evidence_dir()
    metadata = json.loads(capture(['docker', 'image', 'inspect', os.environ['SMOKE_IMAGE']]))[0]
    labels = metadata['Config']['Labels']
    for key, expected in {**evidence_module().EXPECTED_LABELS,
                         'org.opencontainers.image.version': os.environ['RELEASE_VERSION'],
                         'org.opencontainers.image.revision': os.environ['RELEASE_REVISION']}.items():
        if labels.get(key) != expected:
            raise ValueError(f'incorrect image label {key}')
    if metadata['Architecture'] != os.environ['EXPECTED_ARCHITECTURE'] or not DIGEST.fullmatch(metadata['Id']):
        raise ValueError('image architecture/identity mismatch')
    subset = {key: metadata[key] for key in ('Id', 'Architecture', 'Os')}
    subset['Config'] = {key: metadata['Config'][key] for key in ('User', 'Labels')}
    (path / 'image-metadata.json').write_text(json.dumps(subset) + '\n')
    (path / 'image-digest.txt').write_text(metadata['Id'] + '\n')
    run([os.environ['SYFT_COMMAND'], os.environ['SMOKE_IMAGE'], '-o', f'spdx-json={path / "sbom.spdx.json"}'])


def archive_image() -> None:
    path = evidence_dir() / 'image.tar.gz'
    # Stream through Python gzip; subprocess status is never hidden by a pipeline.
    with path.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', compresslevel=1, mtime=0) as compressed:
        with subprocess.Popen(['docker', 'save', os.environ['SMOKE_IMAGE']], cwd=ROOT, stdout=subprocess.PIPE) as process:
            assert process.stdout is not None
            shutil.copyfileobj(process.stdout, compressed)
            if process.wait() != 0:
                raise RuntimeError('docker save failed')


def finalize_evidence() -> None:
    path = evidence_dir()
    module = evidence_module()
    architecture = os.environ['EXPECTED_ARCHITECTURE']
    for report in ('trivy-critical.json', 'trivy-high.json'):
        source = ROOT / report
        if source.exists():
            module.require_regular_file(source)
            shutil.copyfile(source, path / report)
    (path / 'scan-status.txt').write_text(f'architecture={architecture}\ncritical_scan={"present" if (path / "trivy-critical.json").is_file() else "unavailable"}\nhigh_scan={"present" if (path / "trivy-high.json").is_file() else "unavailable"}\n')
    version, revision = os.environ['RELEASE_VERSION'], os.environ['RELEASE_REVISION']
    metadata, labels = module.validate_inputs(path, architecture, version, revision)
    module.write_manifest(path, architecture, version, revision, metadata, labels)
    names = (*module.REQUIRED_FILES, module.MANIFEST_NAME)
    (path / module.CHECKSUMS_NAME).write_text(''.join(f'{module.digest(path / name)}  {name}\n' for name in names))
    module.validate_manifest(path, architecture, version, revision)


def load_images() -> None:
    module = evidence_module()
    for architecture in ('amd64', 'arm64'):
        path = Path(os.environ['EVIDENCE_ROOT']) / f'melodee-release-evidence-{architecture}'
        if module.contains_symlink_component(path):
            raise ValueError('evidence directory must not traverse symlinks')
        module.validate_manifest(path, architecture, os.environ['RELEASE_VERSION'], os.environ['RELEASE_REVISION'])
        run(['docker', 'load', '--input', str(path / 'image.tar.gz')])
        image_id = (path / 'image-digest.txt').read_text().strip()
        if not DIGEST.fullmatch(image_id):
            raise ValueError('invalid image digest')
        run(['docker', 'tag', image_id, f'melodee-scanned:{architecture}'])
        if capture(['docker', 'image', 'inspect', '--format', '{{.Id}}', f'melodee-scanned:{architecture}']) != image_id:
            raise ValueError('loaded image differs from scanned image')


def remote_digest(reference: str) -> str | None:
    result = subprocess.run(['docker', 'buildx', 'imagetools', 'inspect', reference, '--format', '{{.Manifest.Digest}}'], cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode:
        # Auth, network and registry errors are not evidence that a tag is absent.
        if re.search(r'(?i)(manifest unknown|name unknown|manifest_unknown|name_unknown)', result.stderr):
            return None
        raise RuntimeError('cannot establish registry tag state; refusing publication')
    digest = result.stdout.strip()
    if not DIGEST.fullmatch(digest):
        raise ValueError('registry returned an invalid digest')
    return digest


def publish_tags(tags: list[str], candidate: str, *, inspect=remote_digest, command=run) -> str:
    version = os.environ['RELEASE_VERSION']
    exact = f'{IMAGE}:{version}'
    if not SEMVER.fullmatch(version) or not tags or tags[0] != exact:
        raise ValueError('first final tag must be the exact validated version')
    allowed = {exact, f'{IMAGE}:{version.split(".")[0]}.{version.split(".")[1]}', f'{IMAGE}:latest'}
    if any(tag not in allowed for tag in tags) or ('-' in version and tags != [exact]):
        raise ValueError('invalid release tag set')
    digest = inspect(candidate)
    if digest is None:
        raise ValueError('candidate manifest is absent')
    existing = inspect(exact)
    if existing is not None and existing != digest:
        raise ValueError('immutable exact-version tag already has a different digest; reuse original qualified artifacts or release a new version')
    # A successful retry is a no-op: never roll floating tags back to an older release.
    if existing == digest:
        return digest
    # Commit the immutable exact tag last. If a floating-tag write fails, the
    # same original artifacts can safely resume before the release is committed.
    for tag in [*tags[1:], exact]:
        command(['docker', 'buildx', 'imagetools', 'create', '--tag', tag, f'{IMAGE}@{digest}'])
        if inspect(tag) != digest:
            raise RuntimeError('published manifest digest differs from the qualified candidate')
    return digest


def execute(task: str) -> None:
    if task == 'validate-version':
        packages = json.loads(capture(['cargo', 'metadata', '--no-deps', '--format-version', '1']))['packages']
        output(**version_metadata(os.environ['RELEASE_TAG'], os.environ['RELEASE_PRERELEASE'], packages))
    elif task == 'record-revision':
        run(['git', 'fetch', 'origin', 'main'])
        run(['git', 'merge-base', '--is-ancestor', 'HEAD', 'origin/main'])
        output(revision=capture(['git', 'rev-parse', 'HEAD']))
    elif task == 'verify-revision':
        revision_matches(os.environ['RELEASE_REVISION'])
    elif task == 'prepare-evidence':
        evidence_dir().mkdir(parents=True, exist_ok=True)
    elif task == 'record-image':
        record_image()
    elif task == 'archive-image':
        archive_image()
    elif task == 'finalize-evidence':
        finalize_evidence()
    elif task == 'require-evidence':
        module = evidence_module()
        module.require_files(evidence_dir(), (*module.REQUIRED_FILES, module.MANIFEST_NAME, module.CHECKSUMS_NAME))
    elif task == 'load-images':
        load_images()
    elif task == 'push-staging':
        for arch in ('amd64', 'arm64'):
            reference = os.environ[f'STAGING_{arch.upper()}']
            if not reference.startswith(IMAGE + ':'):
                raise ValueError('unexpected staging registry')
            run(['docker', 'tag', f'melodee-scanned:{arch}', reference])
            run(['docker', 'push', reference])
    elif task == 'publish':
        run_id, attempt = os.environ['GITHUB_RUN_ID'], os.environ['GITHUB_RUN_ATTEMPT']
        if not run_id.isdecimal() or not attempt.isdecimal():
            raise ValueError('invalid workflow run identity')
        candidate = f'{IMAGE}:{os.environ["RELEASE_VERSION"]}-candidate-{run_id}-{attempt}'
        run(['docker', 'buildx', 'imagetools', 'create', '--tag', candidate, os.environ['STAGING_AMD64'], os.environ['STAGING_ARM64']])
        output(digest=publish_tags(os.environ['FINAL_TAGS'].splitlines(), candidate))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('task', choices=('validate-version', 'record-revision', 'verify-revision', 'prepare-evidence', 'record-image', 'archive-image', 'finalize-evidence', 'require-evidence', 'load-images', 'push-staging', 'publish'))
    args = parser.parse_args()
    try:
        execute(args.task)
    except (ValueError, KeyError, RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f'Release operation failed: {error}', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
