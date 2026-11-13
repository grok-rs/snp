"""py_vs_lean (ADR-095, docs/VERIFICATION.md §4.3, TCB row 14): the Lean reference models against the Python they
transliterate, so the models stay the upstream behaviour the proofs and the Rust oracle tests are anchored to.

* `shlex-model` (K4, proofs/lean/SnpModels/Shlex.lean) against CPython's `shlex.split`, `shlex.quote` and
  `shlex.join` (`Lib/shlex.py`); the design pins CPython 3.12 (`--expect-python 3.12`).
* `eof-model` (K3, proofs/lean/SnpModels/Eof.lean) against pre-commit-hooks' `end_of_file_fixer.fix_file`
  (`end_of_file_fixer.py:9-49`) at the pinned tag v6.0.0, loaded by file path from a checkout.

Nightly only (per-PR CI stays free of Python, §13.3). Run it isolated, after `lake build` in proofs/lean:

    git clone --depth 1 --branch v6.0.0 https://github.com/pre-commit/pre-commit-hooks target/pch-v6.0.0
    python3 -I xtask/oracle/py_vs_lean.py --bin proofs/lean/.lake/build/bin --pch target/pch-v6.0.0 \
        --expect-python 3.12 [--rounds 10000] [--seed 1] [--only shlex|eof]

Inputs: every string over {' ', '\\t', "'", '"', '\\\\', 'a'} up to length 7 and seeded random strings (wider
alphabet, non-ASCII, NUL) for split; every string up to length 5 over a quoting alphabet plus the random strings
for quote; seeded random argument lists for join; every file over {a, \\n, \\r} up to 9 bytes plus seeded random
files for end-of-file-fixer. Documented gap: Python `str` admits lone surrogates, Lean `Char` and Rust `char` do
not, so no input contains one. Exit status 0 when everything agrees, 1 on a mismatch (the first ones are printed
with their seed), 2 on a usage or environment error.
"""
from __future__ import annotations

import argparse
import configparser
import hashlib
import importlib.util
import io
import itertools
import random
import shlex
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable

PCH_VERSION = '6.0.0'
SPLIT_EXHAUSTIVE = [' ', '\t', "'", '"', '\\', 'a']
SPLIT_RANDOM = [' ', '\t', '\r', '\n', "'", '"', '\\', 'a', 'b', '#', '$', '\0', 'é', '\x85', '\xa0', '😀']
QUOTE_ALPHABET = ['a', '_', '/', "'", '"', ' ', '\\', 'é']
MAX_REPORTED = 10


class Model:
    """One oracle executable in `--lines` mode (cleared environment, one hex line per request)."""

    def __init__(self, exe: Path) -> None:
        self.name = exe.name
        self.proc = subprocess.Popen(
            [str(exe), '--lines'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env={},
            text=True, encoding='ascii', bufsize=1,
        )

    def ask(self, line: str) -> str:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(line + '\n')
        self.proc.stdin.flush()
        reply = self.proc.stdout.readline()
        if not reply:
            raise RuntimeError(f'{self.name} closed its output')
        return reply.rstrip('\n')

    def close(self) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.close()
        self.proc.wait()


class Tally:
    """Counts agreeing cases and keeps the first mismatches."""

    def __init__(self) -> None:
        self.cases = 0
        self.mismatches: list[str] = []

    def check(self, what: str, got: str, want: str, context: str) -> None:
        self.cases += 1
        if got != want and len(self.mismatches) < MAX_REPORTED:
            self.mismatches.append(f'{what}: model {got!r} != python {want!r} ({context})')
        elif got != want:
            self.mismatches.append('')


def tokens_reply(ts: list[str]) -> str:
    """The `shlex-model` framing: `OK <n> :<hex utf-8>…`."""
    return f'OK {len(ts)}' + ''.join(' :' + t.encode().hex() for t in ts)


def py_split(s: str) -> str:
    try:
        return tokens_reply(shlex.split(s))
    except ValueError as e:
        return f'ERR {e}'


def check_shlex(bin_dir: Path, rounds: int, seed: int, tally: Tally) -> None:
    model = Model(bin_dir / 'shlex-model')
    rng = random.Random(seed)
    for k in range(8):
        for tup in itertools.product(SPLIT_EXHAUSTIVE, repeat=k):
            s = ''.join(tup)
            tally.check('split', model.ask('split ' + s.encode().hex()), py_split(s), f'exhaustive {s!r}')
    for k in range(6):
        for tup in itertools.product(QUOTE_ALPHABET, repeat=k):
            s = ''.join(tup)
            tally.check('quote', model.ask('quote ' + s.encode().hex()), tokens_reply([shlex.quote(s)]), repr(s))
    for r in range(rounds):
        s = ''.join(rng.choice(SPLIT_RANDOM) for _ in range(rng.randrange(0, 49)))
        context = f'seed {seed}, round {r}, {s!r}'
        tally.check('split', model.ask('split ' + s.encode().hex()), py_split(s), context)
        tally.check('quote', model.ask('quote ' + s.encode().hex()), tokens_reply([shlex.quote(s)]), context)
        args = [''.join(rng.choice(SPLIT_RANDOM) for _ in range(rng.randrange(0, 9))) for _ in range(rng.randrange(5))]
        request = f'join {len(args)}' + ''.join(' :' + a.encode().hex() for a in args)
        tally.check('join', model.ask(request), tokens_reply([shlex.join(args)]), f'seed {seed}, round {r}, {args!r}')
        # The proved round trip (`split_join`), on the Python side.
        tally.check('split(join)', repr(shlex.split(shlex.join(args))), repr(args), f'seed {seed}, round {r}')
    model.close()


def load_fix_file(pch: Path) -> Callable[[io.BytesIO], int]:
    """`end_of_file_fixer.fix_file` from a pre-commit-hooks checkout, after checking its version."""
    cfg = configparser.ConfigParser()
    cfg.read(pch / 'setup.cfg', encoding='utf-8')
    version = cfg.get('metadata', 'version', fallback='?')
    if version != PCH_VERSION:
        sys.exit(f'error: {pch} is pre-commit-hooks {version}, the pinned tag is v{PCH_VERSION}')
    source = pch / 'pre_commit_hooks' / 'end_of_file_fixer.py'
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    print(f'pre-commit-hooks {version}: {source.name} sha256 {digest}')
    spec = importlib.util.spec_from_file_location('end_of_file_fixer', source)
    if spec is None or spec.loader is None:
        sys.exit(f'error: cannot load {source}')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.fix_file


def eof_reply(reply: str) -> str:
    """`<code> <hex>` from an `eof-model` reply, tolerating the `OK` prefix and `:` hex marker of the shared
    convention (`OK …` / `ERR …`, proofs/lean/client.rs)."""
    fields = reply.split()
    if fields and fields[0] == 'OK':
        fields = fields[1:]
    if not fields or fields[0] == 'ERR':
        return reply
    hex_out = fields[-1].lstrip(':') if len(fields) > 1 else ''
    return f'{fields[0]} {hex_out}'


def check_eof(bin_dir: Path, pch: Path, rounds: int, seed: int, tally: Tally) -> None:
    fix_file = load_fix_file(pch)

    def python(data: bytes) -> str:
        f = io.BytesIO(data)
        rc = fix_file(f)
        return f'{rc} {f.getvalue().hex()}'

    model = Model(bin_dir / 'eof-model')
    rng = random.Random(seed)
    for k in range(10):
        for tup in itertools.product(b'a\n\r', repeat=k):
            d = bytes(tup)
            tally.check('end-of-file-fixer', eof_reply(model.ask(d.hex())), python(d), f'exhaustive {d!r}')
    for r in range(rounds):
        d = bytes(rng.choice(b'\n\r\n\rab\x00\xff') for _ in range(rng.randrange(0, 40)))
        tally.check('end-of-file-fixer', eof_reply(model.ask(d.hex())), python(d), f'seed {seed}, round {r}, {d!r}')
    model.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--bin', type=Path, required=True, help='the built oracles (proofs/lean/.lake/build/bin)')
    parser.add_argument('--pch', type=Path, help=f'a pre-commit-hooks v{PCH_VERSION} checkout (needed for eof)')
    parser.add_argument('--rounds', type=int, default=10000, help='seeded random cases per model (default 10000)')
    parser.add_argument('--seed', type=int, default=1)
    parser.add_argument('--only', choices=['shlex', 'eof'])
    parser.add_argument('--expect-python', metavar='X.Y', help='fail unless this CPython minor runs the script')
    args = parser.parse_args()

    running = f'{sys.version_info.major}.{sys.version_info.minor}'
    if args.expect_python and running != args.expect_python:
        print(f'error: CPython {running} runs this script, expected {args.expect_python}', file=sys.stderr)
        return 2
    if args.only != 'shlex' and args.pch is None:
        print('error: --pch is required unless --only shlex', file=sys.stderr)
        return 2

    failed = False
    for name in ('shlex', 'eof'):
        if args.only not in (None, name):
            continue
        tally = Tally()
        start = time.monotonic()
        if name == 'shlex':
            check_shlex(args.bin, args.rounds, args.seed, tally)
            against = f'CPython {sys.version.split()[0]} shlex'
        else:
            check_eof(args.bin, args.pch, args.rounds, args.seed, tally)
            against = f'pre-commit-hooks v{PCH_VERSION} end_of_file_fixer'
        elapsed = time.monotonic() - start
        if tally.mismatches:
            failed = True
            print(f'{name}: {len(tally.mismatches)} of {tally.cases} cases differ from {against}:')
            for m in tally.mismatches:
                if m:
                    print(f'  {m}')
        else:
            print(f'{name}: {tally.cases} cases agree with {against} in {elapsed:.2f}s')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
