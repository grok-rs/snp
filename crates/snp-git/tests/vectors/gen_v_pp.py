"""Generate the V-PP pre-push stdin vectors (15 §A V-PP, 02 §16.3, ADR-022) with the pre-commit 4.6.2 oracle.

Run with the oracle interpreter (never a PATH python):

    target/oracle/pc462-py312/bin/python -I crates/snp-git/tests/vectors/gen_v_pp.py > crates/snp-git/tests/vectors/v_pp.json

For every stdin it records what pre-commit's `hook_impl._pre_push_ns` parses, line by line, until the first error
(`stdin.decode().splitlines()` + `line.rsplit(maxsplit=3)` + 4-way unpack), and what snp parses: the same for valid
UTF-8 and, where pre-commit raises `UnicodeDecodeError`, the same algorithm on bytes (D-CRASH-11:
`bytes.splitlines()` + `bytes.rsplit(maxsplit=3)`). The replica of the loop is checked against the oracle's source so
it cannot drift. Owner: WP-18.
"""
from __future__ import annotations

import inspect
import json
import sys

from pre_commit.commands import hook_impl

SRC = inspect.getsource(hook_impl._pre_push_ns)
assert 'for line in stdin.decode().splitlines():' in SRC, SRC
assert 'parts = line.rsplit(maxsplit=3)' in SRC, SRC
assert 'local_branch, local_sha, remote_branch, remote_sha = parts' in SRC, SRC
assert hook_impl.Z40 == '0' * 40

O1 = '1' * 40
O2 = '2' * 40
Z40 = hook_impl.Z40
Z64 = '0' * 64

INPUTS: list[bytes] = [
    f'refs/heads/main {O1} refs/heads/main {O2}\n'.encode(),
    f'refs/heads/a {O1} refs/heads/a {Z40}\nrefs/heads/b {O2} refs/heads/b {Z40}\n'.encode(),
    f'(delete) {Z40} refs/heads/gone {O2}\n'.encode(),
    f'HEAD@{{1 day ago}} {O1} refs/heads/x {Z40}\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main {O2}\r\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main {O2}'.encode(),
    b'',
    b'\n',
    f'refs/heads/main {O1} refs/heads/main {O2}\n\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main\n'.encode(),
    f'refs/heads/café {O1} refs/heads/x {Z40}\n'.encode(),
    f'refs/heads/caf\xe9 {O1} refs/heads/x {Z40}\n'.encode('latin-1'),
    f'refs/heads/a b {O1} refs/heads/a b {Z40}\n'.encode(),
    f'refs/heads/a\x85b {O1} refs/heads/a\x85b {Z40}\n'.encode('latin-1'),
    f'refs/heads/a\u0085b {O1} refs/heads/a\u0085b {Z40}\n'.encode(),
    f'refs/heads/a\xa0b {O1} refs/heads/a\xa0b {Z40}\n'.encode(),
    f'refs/heads/a　b {O1} refs/heads/x {Z40}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/a\x1fb {Z40}\n'.encode(),
    f'  refs/heads/x {O1} refs/heads/x {Z40}  \n'.encode(),
    f'refs/heads/x\t{O1}\trefs/heads/x\t{Z40}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/x {Z40}\x0crefs/heads/y {O2} refs/heads/y {Z40}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/x {Z40}\rrefs/heads/y {O2} refs/heads/y {Z40}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/x {Z40}\x1crefs/heads/y {O2} refs/heads/y {Z40}\n'.encode(),
    f'(delete) {Z64} refs/heads/gone {O2}\n'.encode(),
    f'(delete) {Z40} refs/heads/gone {O2}\n\n'.encode(),
    f'a b c d e f\n'.encode(),
    b'refs/heads/\xff\xfe x\xff refs/heads/y \x00\n',
    b'\xe2\x82\n',
    f'refs/heads/x {O1} refs/heads/x {Z40}\nbad line\n'.encode(),
    b'\xff\nrefs/heads/x' + O1.encode() + b'\n',
]


def fields(parts: list) -> dict:
    if len(parts) != 4:
        return {'error': f'ValueError: not enough values to unpack (expected 4, got {len(parts)})'}
    return {'fields': [p.hex() if isinstance(p, bytes) else p.encode().hex() for p in parts]}


def str_records(text: str) -> list[dict]:
    out = []
    for line in text.splitlines():
        rec = fields(line.rsplit(maxsplit=3))
        out.append(rec)
        if 'error' in rec:
            break
    return out


def bytes_records(raw: bytes) -> list[dict]:
    out = []
    for line in raw.splitlines():
        rec = fields(line.rsplit(maxsplit=3))
        out.append(rec)
        if 'error' in rec:
            break
    return out


def main() -> int:
    vectors = []
    for raw in INPUTS:
        try:
            text = raw.decode()
        except UnicodeDecodeError as e:
            pc = [{'error': f'UnicodeDecodeError: {e}'}]
            snp = bytes_records(raw)
        else:
            pc = str_records(text)
            snp = pc
        vectors.append({'stdin_hex': raw.hex(), 'pre_commit': pc, 'snp': snp})
    json.dump(
        {
            'source': 'pre-commit 4.6.2 hook_impl._pre_push_ns (oracle pc462-py312)',
            'python': sys.version.split()[0],
            'vectors': vectors,
        },
        sys.stdout,
        indent=1,
    )
    sys.stdout.write('\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
