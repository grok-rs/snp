"""Fetch the conformance corpora snp-pyparsers' vectors are generated from (WP-17), pinned by commit and sha256.

usage: python3 -I pch_corpora.py DIR      (default DIR: target/pch-corpora)

Creates DIR/JSONTestSuite, DIR/yaml-test-suite, DIR/toml-test and DIR/xmlconf from the archives below; an archive
whose sha256 differs is rejected. Only `pch_vectors.py` reads the corpora; the generated vectors carry every input
they need, so neither CI nor the tests download anything.
"""
from __future__ import annotations

import hashlib
import io
import os
import shutil
import sys
import tarfile
import urllib.request

ARCHIVES = [
    # (directory, url, sha256, top-level directory inside the archive)
    (
        'JSONTestSuite',
        'https://codeload.github.com/nst/JSONTestSuite/tar.gz/1ef36fa01286573e846ac449e8683f8833c5b26a',
        '32ab2fb37cc26679dbd9164dbb7053bcef10134643d53cb0cef45de7680dcdcc',
        'JSONTestSuite-1ef36fa01286573e846ac449e8683f8833c5b26a',
    ),
    (
        'yaml-test-suite',
        'https://codeload.github.com/yaml/yaml-test-suite/tar.gz/6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f',
        'cc4a08f9ccc1cb2e66f32b3f1192bf1f07b3175682bbb715bf709bafa70322d4',
        'yaml-test-suite-6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f',
    ),
    (
        'toml-test',
        'https://codeload.github.com/toml-lang/toml-test/tar.gz/f0b5cddb0e7299fe29f2b881f383ea932950fba3',
        '960cc4d2b73886e882e16d8c1479f809767d708269339fed741ad82c2c567166',
        'toml-test-f0b5cddb0e7299fe29f2b881f383ea932950fba3',
    ),
    (
        'xmlconf',
        'https://www.w3.org/XML/Test/xmlts20130923.tar.gz',
        '9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f',
        'xmlconf',
    ),
]


def safe_members(tar: tarfile.TarFile, top: str) -> list[tarfile.TarInfo]:
    out = []
    for m in tar.getmembers():
        name = m.name
        if name.startswith('/') or '..' in name.split('/') or not (name == top or name.startswith(top + '/')):
            raise SystemExit(f'unsafe archive member {name!r}')
        if m.issym() or m.islnk() or m.isdev():
            continue
        out.append(m)
    return out


def main() -> int:
    dest = sys.argv[1] if len(sys.argv) > 1 else os.path.join('target', 'pch-corpora')
    os.makedirs(dest, exist_ok=True)
    for name, url, sha, top in ARCHIVES:
        target = os.path.join(dest, name)
        if os.path.isdir(target):
            print(f'{target}: present', file=sys.stderr)
            continue
        with urllib.request.urlopen(url, timeout=300) as r:
            data = r.read()
        digest = hashlib.sha256(data).hexdigest()
        if sha is not None and digest != sha:
            raise SystemExit(f'{url}: sha256 {digest} != {sha}')
        tmp = target + '.tmp'
        shutil.rmtree(tmp, ignore_errors=True)
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as tar:
            tar.extractall(tmp, members=safe_members(tar, top), filter='data')
        os.replace(os.path.join(tmp, top), target)
        shutil.rmtree(tmp, ignore_errors=True)
        print(f'{target}: {url} (sha256 {digest})', file=sys.stderr)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
