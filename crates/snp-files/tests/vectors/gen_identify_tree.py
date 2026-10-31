"""Generate `identify_tree.json`: identify 2.6.20's `tags_from_path` over a generated tree.

Run with the oracle's interpreter (pre-commit 4.6.2 pins identify 2.6.20), from any directory:

    target/oracle/pc462-py312/bin/python -I crates/snp-files/tests/vectors/gen_identify_tree.py \
        > crates/snp-files/tests/vectors/identify_tree.json

The tree is defined below; `crates/snp-files/tests/identify_oracle.rs` materialises the same entries and compares
snp-files' tags with the recorded ones. Every entry is created by the current user with a mode whose owner `x` bit
equals "any `x` bit", so the expected `executable` tag is the same for a non-root and a root test run (identify uses
`os.access(X_OK)`). FIFOs are fed by a writer thread that opens and closes the write end whenever identify blocks in
`open()` (identify would otherwise hang, X13-CLS-03); the reader then sees an empty FIFO, which is what snp's
non-blocking open sees.

Owner: WP-22.
"""
from __future__ import annotations

import json
import os
import socket
import sys
import tempfile
import threading
import time

from identify import identify

TEXT = b'hello\n'
BIN = b'\x00\x01\x02binary'


def reg(name, content=TEXT, mode=0o644):
    return {'path': name, 'kind': 'file', 'mode': mode, 'content': content}


def exe(name, content, mode=0o755):
    return reg(name, content, mode)


ENTRIES = [
    # Names and extensions (no content needed when the name decides).
    reg(b'setup.py'),
    reg(b'a.PY'),
    reg(b'README.md'),
    reg(b'Makefile'),
    reg(b'Makefile.png', BIN),
    reg(b'Dockerfile.xenial'),
    reg(b'.bashrc'),
    reg(b'.pre-commit-config.yaml'),
    reg(b'x.yml'),
    reg(b'x.json'),
    reg(b'a.tar.gz', BIN),
    reg(b'image.jpg', BIN),
    reg(b'photo.JPG', BIN),
    reg(b'LICENSE'),
    reg(b'COPYING'),
    reg(b'CMakeLists.txt'),
    reg(b'go.mod'),
    reg(b'a.pyi'),
    reg(b'x.sh'),
    reg(b'x.bat'),
    reg(b'a.c'),
    reg(b'nb.ipynb'),
    reg(b'a..b'),
    reg(b'..foo'),
    reg(b'foo.'),
    reg(b'x.svg'),
    reg(b'x.plist', BIN),
    reg(b'x.plist.text', TEXT),
    reg(b'y.plist', TEXT),
    reg(b'bad\xffname.py'),
    reg(b'bad\xffname'),
    reg(b'\xc3\xa9t\xc3\xa9.py'),
    reg(b'sub/dir/deep.rs'),
    # Unknown names: the first 1024 bytes decide text/binary.
    reg(b'noext'),
    reg(b'empty', b''),
    reg(b'data.unknownext', BIN),
    reg(b'latin1.unknownext', bytes(range(0x80, 0x100))),
    reg(b'ctrl.unknownext', b'abc\x01def'),
    reg(b'escape.unknownext', b'a\x1b[31mred\x1b[0m\x07\x08\x0b\x0c\r\n'),
    reg(b'nul_at_1023', b'a' * 1023 + b'\x00'),
    reg(b'nul_at_1024', b'a' * 1024 + b'\x00'),
    reg(b'shebang_not_exec', b'#!/usr/bin/env python3\n'),
    # Executables without name tags: shebang tags (+ encoding).
    exe(b'env_python', b'#!/usr/bin/env python3\nprint(1)\n'),
    exe(b'env_s', b'#!/usr/bin/env -S python3 -u\n'),
    exe(b'env_s_alone', b'#!/usr/bin/env -S\n'),
    exe(b'env_alone', b'#!/usr/bin/env\n'),
    exe(b'bare_env', b'#!env python\n'),
    exe(b'abs_python', b'#!/usr/bin/python3.12\n'),
    exe(b'python_dotted', b'#!/usr/bin/python3.12.1\n'),
    exe(b'bash', b'#!/bin/bash\necho\n'),
    exe(b'space_sh', b'#! /bin/sh\n'),
    exe(b'crlf', b'#!/usr/bin/env bash\r\necho\r\n'),
    exe(b'nix', b'#!/usr/bin/env nix-shell\n#!nix-shell -i python3 -p foo\nprint(1)\n'),
    exe(b'nix_last_wins', b'#!/usr/bin/env nix-shell\n#! nix-shell -i bash\n#! nix-shell -i python3\n'),
    exe(b'nix_plain', b'#!nix-shell\n#!nix-shell -i ruby\n'),
    exe(b'nix_no_i', b'#!/usr/bin/env nix-shell\n#!nix-shell -p foo\n'),
    exe(b'quoted', b'#!"/usr/bin/my python" -x\n'),
    exe(b'env_quoted', b"#!/usr/bin/env 'python3'\n"),
    exe(b'unclosed_quote', b'#!/usr/bin/env "python3\n'),
    exe(b'nonprintable', b'#!/usr/bin/python\x01\n'),
    exe(b'not_utf8', b'#!/usr/bin/pyth\xffon\n'),
    exe(b'long_unknown', b'#!/usr/bin/env ' + b'a' * 2000 + b'\n'),
    exe(b'long_python', b'#!/usr/bin/python3 ' + b'-x ' * 600 + b'\n'),
    exe(b'only_bang', b'#!'),
    exe(b'only_hash', b'#'),
    exe(b'exec_empty', b''),
    exe(b'exec_binary', b'\x7fELF\x02\x01\x01\x00\x00\x00'),
    exe(b'node', b'#!/usr/local/bin/node\n'),
    exe(b'ruby', b'#!/usr/bin/env ruby\n'),
    exe(b'perl', b'#!/usr/bin/perl -w\n'),
    exe(b'tab_sep', b'#!/usr/bin/env\tpython3\n'),
    exe(b'run.py', b'#!/bin/bash\n'),
    exe(b'tool.unknownext', b'#!/usr/bin/env python3\n'),
    exe(b'owner_only', b'#!/bin/sh\n', 0o700),
    exe(b'all_x', b'#!/bin/sh\n', 0o711),
    exe(b'owner_x_others_r', b'#!/bin/sh\n', 0o744),
    reg(b'no_x_anywhere', b'#!/bin/sh\n', 0o600),
    # Directories, symlinks, sockets, FIFOs.
    {'path': b'subdir', 'kind': 'dir'},
    {'path': b'dir.py', 'kind': 'dir'},
    {'path': b'link.py', 'kind': 'symlink', 'target': b'setup.py'},
    {'path': b'broken.py', 'kind': 'symlink', 'target': b'nowhere'},
    {'path': b'dirlink', 'kind': 'symlink', 'target': b'subdir'},
    {'path': b'sock', 'kind': 'socket'},
    {'path': b'sock.py', 'kind': 'socket'},
    {'path': b'fifo', 'kind': 'fifo', 'mode': 0o644},
    {'path': b'fifo.py', 'kind': 'fifo', 'mode': 0o644},
    {'path': b'fifo_exec', 'kind': 'fifo', 'mode': 0o755},
]


def materialize(root: str) -> list[socket.socket]:
    socks = []
    for e in ENTRIES:
        p = os.path.join(root.encode(), e['path'])
        os.makedirs(os.path.dirname(p), exist_ok=True)
        kind = e['kind']
        if kind == 'file':
            with open(p, 'wb') as f:
                f.write(e['content'])
            os.chmod(p, e['mode'])
        elif kind == 'dir':
            os.makedirs(p, exist_ok=True)
        elif kind == 'symlink':
            os.symlink(e['target'], p)
        elif kind == 'fifo':
            os.mkfifo(p)
            os.chmod(p, e['mode'])
        elif kind == 'socket':
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.bind(os.fsdecode(e['path']))
            socks.append(s)
        else:
            raise AssertionError(kind)
    return socks


def feed_fifos(stop: threading.Event) -> None:
    fifos = [e['path'] for e in ENTRIES if e['kind'] == 'fifo']
    while not stop.is_set():
        for p in fifos:
            try:
                fd = os.open(p, os.O_WRONLY | os.O_NONBLOCK)
            except OSError:
                continue
            os.close(fd)
        time.sleep(0.0005)


def main() -> int:
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        os.chdir(tmp)
        socks = materialize('.')
        stop = threading.Event()
        feeder = threading.Thread(target=feed_fifos, args=(stop,), daemon=True)
        feeder.start()
        try:
            for e in ENTRIES:
                tags = sorted(identify.tags_from_path(os.fsdecode(e['path'])))
                rec = {
                    'path': e['path'].hex(),
                    'display': e['path'].decode('utf-8', 'backslashreplace'),
                    'kind': e['kind'],
                }
                if 'mode' in e:
                    rec['mode'] = e['mode']
                if 'content' in e:
                    rec['content'] = e['content'].hex()
                if 'target' in e:
                    rec['target'] = e['target'].hex()
                rec['tags'] = tags
                out.append(rec)
        finally:
            stop.set()
            feeder.join()
            for s in socks:
                s.close()
            os.chdir('/')
    import identify as identify_pkg  # noqa: F401
    from importlib.metadata import version
    json.dump(
        {
            'generator': 'crates/snp-files/tests/vectors/gen_identify_tree.py',
            'identify': version('identify'),
            'python': sys.version.split()[0],
            'entries': out,
        },
        sys.stdout,
        indent=1,
    )
    sys.stdout.write('\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
