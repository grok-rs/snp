"""Generate parity test vectors for file-identify from Python identify.

This script IMPORTS and RUNS Python identify, so point it at a copy of the
identify sources you trust, and run it isolated:

    python3 -I xtask/oracle/gen_identify_vectors.py \
        --identify /path/to/copy/of/identify-2.6.20 \
        --out crates/snp-identify/tests/data/identify-vectors.json

``cargo xtask identify-vectors`` runs it under the pre-commit 4.7.0 oracle interpreter
(target/oracle/pc470-py312), whose site-packages hold the hash-locked identify 2.6.20.

`--identify` is the directory that contains the `identify/` package and
either `setup.cfg` (source checkout) or `identify-*.dist-info/` (unpacked
wheel or installed copy, e.g. a venv's site-packages), used for the version.
The output is consumed by snp-identify's `tests/parity.rs`. Everything is deterministic (fixed
random seed).
"""
from __future__ import annotations

import argparse
import configparser
import io
import json
import ntpath
import os
import random
import shlex
import stat
import sys
import tempfile

SEED = 20261008


def hexb(b: bytes) -> str:
    return b.hex()


def gen_filenames(rng, identify, extensions):
    names = set()
    names.update([
        '', '.', '..', '...', '.py', '..py', 'a.', 'a..b', 'x.tar.gz',
        'Makefile.png', 'Dockerfile.xenial', 'xenial.Dockerfile', '.bashrc',
        'foo.bashrc', 'a\\b\\c.py', 'C:foo.py', 'C:\\x\\y.rs', 'c:/x/y.rs',
        '\\\\server\\share', '\\\\server\\share\\x.py', '//server/share/x.py',
        '//server', '\\\\?\\UNC\\srv\\share\\z.go', '\\Windows', 'dir/', 'dir/.',
        'x.\u212at', 'x.\u0130ni', 'x.PY', 'X.Py', '\u00fcn\u00efc\u00f6d\u00e9.md',
        'x.py ', ' x.py', 'x .py', 'README', 'README.md', 'readme.md',
        'Dockerfile', 'dockerfile', 'mod/Dockerfile', 'mod/test.py',
        'setup.cfg', 'test.cfg', 'f.plist', 'f.JPG', 'dockerfile.py',
        'Tiltfile.abc', 'test.Tiltfile', 'meson_options.txt', '.eslintrc',
        '.prettierrc', 'yarn.lock', 'package.json', 'composer.lock',
        'BUILD.bazel', 'WORKSPACE.bazel', 'pyproject.toml', 'uv.lock',
        'Pipfile.lock', 'x.\u00e9', 'x.\u212a', 'x.' + 'a' * 40,
        'a.b.c.d.e.f', 'Makefile.am.in', 'CMakeLists.txt', '.gitignore',
        'x.c++', 'x.C++', 'x.cs', 'x.CS', 'archive.ZIP', 'Gemfile.lock.bak',
    ])
    # Windows drives: ntpath.splitroot tests the second *character* for ':',
    # so the drive "letter" may be any (also non-ASCII) character.
    for drive in ['C', 'c', 'z', '1', ' ', ':', '.', '\u00e9', '\u20ac', '\U0001f600',
                  '\u212a', '\u0130', '\u00e9\u00e9', 'ab']:
        for sep in ['', '\\', '/', '\\\\', ':']:
            for tail in ['x.py', 'Dockerfile', '.sqlfluff', 'bblayers.conf', 'a\\b.rs',
                         'a/b.go', 'Containerfile.hgrcasciidoc', '']:
                names.add(drive + ':' + sep + tail)
    names.update([
        '\u00e9\\:x.py', '\u00e9/:x.py', '\\\\\u00e9:\\share\\x.py', '\\\u00e9:x.py',
        '\\\\?\\UNC\\\u00e9:\\share\\Makefile', '\\\\?\\c:\\x.py', '\\\\.\\\u00e9:\\x.py',
    ])
    for key in extensions.NAMES:
        names.update([key, 'dir/' + key, key + '.bak', 'x.' + key, 'pre.' + key + '.post'])
        names.add(key.upper())
    for ext in list(extensions.EXTENSIONS) + list(extensions.EXTENSIONS_NEED_BINARY_CHECK):
        names.update(['f.' + ext, 'f.' + ext.upper(), 'F.' + ext.title(), 'a/b.' + ext, '.' + ext])
    pieces = (
        list(extensions.NAMES) + list(extensions.EXTENSIONS)[:120]
        + ['', 'x', 'Foo', 'README', 'py', 'PY', 'Txt', '\u00e9', '\u212a']
    )
    for _ in range(1500):
        parts = [rng.choice(pieces) for _ in range(rng.randint(1, 4))]
        name = '.'.join(parts)
        r = rng.random()
        if r < 0.15:
            name = 'sub/' + name
        elif r < 0.25:
            name = 'sub\\' + name
        elif r < 0.3:
            name = 'C:' + name
        names.add(name)
    out = []
    for name in sorted(names):
        posix = sorted(identify.tags_from_filename(name))
        # On Windows identify uses ntpath.split; the tail has no separators,
        # so splitext behaves identically.
        nt = sorted(identify.tags_from_filename(ntpath.basename(name)))
        # Third element only when the Windows result differs.
        out.append([name, posix] if nt == posix else [name, posix, nt])
    return out


def gen_interpreters(rng, identify, interpreters):
    items = set(['', '.', '/', '//', 'python', 'python3.', '.python', 'python.3',
                 'PYTHON', 'Python3', '/usr/bin/', '/usr/bin/env', 'a/b/python3.12.1',
                 'python3.12.1', 'python3..12', 'bash.exe', 'C:\\python3', 'pypy3',
                 'node.js', 'nodejs.18', 'php8.2', 'php9', 'nu', 'nushell.0'])
    for key in interpreters.INTERPRETERS:
        items.update([key, '/usr/bin/' + key, key + '.1', key + '.1.2', key.upper(),
                      'x' + key, key + 'x', '/opt/' + key + '/bin/' + key + '.9'])
    for _ in range(500):
        k = rng.choice(list(interpreters.INTERPRETERS))
        suffix = ''.join(rng.choice('.0123456789ab-/') for _ in range(rng.randint(0, 5)))
        items.add(k + suffix)
    return [[s, sorted(identify.tags_from_interpreter(s))] for s in sorted(items)]


SHEBANG_TEMPLATES = [
    b'', b'#', b'#!', b'#!\n', b'#! ', b'#!   \n', b'#!/bin/sh', b'#!/bin/sh\n', b'#!/bin/sh -e\n',
    b'#!/bin/sh\r\n', b'#!/bin/sh\r', b'#! /bin/sh \t\n', b'#!\t/bin/bash\t-x\n',
    b'#!/usr/bin/env python3\n', b'#!/usr/bin/env  python3  -u\n', b'#!/usr/bin/env\n',
    b'#!/usr/bin/env -S\n', b'#!/usr/bin/env -S python3 -u -W ignore\n', b'#!/usr/bin/env -Spython\n',
    b'#!env python\n', b'#!/bin/env python\n', b'#!/usr/local/bin/env python\n',
    b"#!/usr/bin/env 'python3'\n", b'#!"/usr/bin/my python" -x\n', b"#!/path'with/quotes y\n",
    b'#!/bin/sh #comment\n', b'#!/usr/bin/env python # c\n', b'#!a\\ b c\n', b'#!"a\\\\b\\q"\n',
    b'#!x\x0by\n', b'#!x\x0cy\n', b'#!x\ry\n', b'#!unterminated "q\n', b'#!trail\\\n', b'#!trail\\',
    b"#!a '' b\n", b"#!''\n", b'#!a"b c"d\n', b'#!/bin/sh\x00\n', b'#!/bin/sh \xc3\xa9\n',
    b'#!\xf9\x93\x01\x42\xcd', b'#!/bin/sh\x7f\n', b'#!/bin/sh\x1b\n', b'#!/bin/sh\x1c\n',
    b'#! /usr/bin/env nix-shell\n#! nix-shell -i bash -p python',
    b'#! /usr/bin/env nix-shell\n#! nix-shell -i python -p coreutils',
    b'#! /usr/bin/env nix-shell\n#! nix-shell -p coreutils -i python',
    b'#! /usr/bin/env nix-shell\n#! nix-shell -p coreutils\n#! nix-shell -i python',
    b'#! /usr/bin/env nix-shell\n#!nix-shell -p coreutils\n#!nix-shell -i python',
    b'#! /usr/bin/env nix-shell\n#!\xf9\x93\x01\x42\xcd',
    b'#! /usr/bin/env nix-shell\n#!\x00\x00\x00\x00',
    b'#! /usr/bin/nix-shell', b'#! /usr/bin/env nix-shell',
    b'#! /usr/bin/env nix-shell non-portable-argument',
    b'#! /usr/bin/env nix-shell\n#! nix-shell -i',
    b'#!/usr/bin/env nix-shell\n#!nix-shell --argstr x "a -i python3 p"\n#!nix-shell -p hello\n'
    b'#!nix-shell -i bash\n#!nix-shell --argstr y "b -i runhaskell q"',
    b'#!nix-shell\n#!x -i zsh\n', b'#!/usr/bin/env -S nix-shell\n#! -i ruby\n',
    b'#!/usr/bin/env nix-shell\n# not a shebang\n#! nix-shell -i bash\n',
    b'#!/usr/bin/env nix-shell\n#! nix-shell -i "my interp" -i\n',
    b'#!/usr/bin/env nix-shell\n#! nix-shell -i unterminated "q\n',
    b'#!/usr/bin/env nix-shell\r\n#! nix-shell -i perl\r\n',
    b'#!/usr/bin/env nix-shell\n#!', b'#!/usr/bin/env nix-shell\n#', b'#!/usr/bin/env nix-shell\n\n#! -i x',
    b'#!/usr/bin/env ' + b' ' * 1500 + b'python3\n',
    b'#!/bin/' + b'x' * 3000 + b'\n',
    b'#!/usr/bin/env nix-shell\n#!' + b' ' * 2000 + b'-i deep\n',
    b'\xef\xbb\xbf#!/bin/sh\n', b' #!/bin/sh\n', b'#!/bin/sh\n#!/bin/bash\n',
]


def gen_shebangs(rng, identify):
    inputs = list(SHEBANG_TEMPLATES)
    alphabet = [b'a', b'b', b' ', b' ', b'\t', b'"', b"'", b'\\', b'-', b'-i', b'/', b'#', b'#!',
                b'\n', b'\n#!', b'\r', b'\x0b', b'\x0c', b'\xc3\xa9', b'\x00', b'env', b'-S',
                b'/usr/bin/env', b'nix-shell', b'python3']
    for _ in range(1500):
        body = b''.join(rng.choice(alphabet) for _ in range(rng.randint(0, 14)))
        prefix = rng.choice([b'#!', b'#!', b'#!', b'#! ', b'#!/usr/bin/env ', b'#!/usr/bin/env nix-shell\n#!', b''])
        inputs.append(prefix + body)
    seen = set()
    out = []
    for b in inputs:
        if b in seen:
            continue
        seen.add(b)
        out.append([hexb(b), list(identify.parse_shebang(io.BytesIO(b)))])
    return out


def gen_shlex(rng):
    inputs = ['', ' ', 'a', 'a b', "'a b'", '"a b"', 'a\\ b', '"a\\"b"', "'a\\'", '"\\\\"',
              '"\\q"', 'a#b', '# x', "''", '""', 'a""b', "a''", "'''", '"', "'", '\\', 'a\\',
              'x\x0by', 'x\x0cy', 'x\ry', 'x\ny', '\u00e9 \u00e8', '"a\nb"', "a 'b c' \"d e\" f\\ g"]
    alphabet = ['a', 'b', ' ', '\t', '\r', '\n', '"', "'", '\\', '#', '\x0b', '\u00e9', '$', '`']
    for _ in range(1500):
        inputs.append(''.join(rng.choice(alphabet) for _ in range(rng.randint(0, 12))))
    out = []
    seen = set()
    for s in inputs:
        if s in seen:
            continue
        seen.add(s)
        try:
            out.append([s, shlex.split(s)])
        except ValueError as e:
            out.append([s, {'error': str(e)}])
    return out


def gen_is_text(rng, identify):
    inputs = [b'', b'hello world', b'hello world\x00', b'\x7f\x45\x4c\x46\x02\x01\x01',
              b'\x43\x92\xd9\x0f\xaf\x32\x2c', bytes(range(256)), b'a' * 1023 + b'\x00',
              b'a' * 1024 + b'\x00', b'a' * 1025 + b'\x00', b'\x1b[0m', b'\x0e', b'\x1f', b'\x7f']
    for b in range(256):
        inputs.append(bytes([b]))
    for _ in range(200):
        n = rng.choice([1, 2, 3, 8, 16, 64])
        weights = rng.random()
        inputs.append(bytes(
            rng.randrange(256) if rng.random() < weights * 0.05 else rng.randrange(0x20, 0x7f)
            for _ in range(n)
        ))
    return [[hexb(b), identify.is_text(io.BytesIO(b))] for b in inputs]


PATH_CONTENTS = {
    'empty': b'',
    'text': b'hello\n',
    'binary': b'\x7fELF\x02\x01\x01\x00',
    'sh': b'#!/bin/sh\necho hi\n',
    'env_py': b'#!/usr/bin/env python3\nprint()\n',
    'env_s': b'#!/usr/bin/env -S python3.12 -u\n',
    'quoted': b'#!"/opt/my tools/ruby" -w\n',
    'nix': b'#! /usr/bin/env nix-shell\n#! nix-shell -i bash -p jq\nset -e\n',
    'nix_bad': b'#! /usr/bin/env nix-shell\n#!\xff\n',
    'incomplete': b'#!   \n',
    'unknown_interp': b'#!/usr/bin/env frobnicate\n',
    'binary_shebang': b'#!/bin/sh\n\x00\x01\x02',
    'long_shebang': b'#!/usr/bin/env ' + b' ' * 1500 + b'node\n',
    'late_nul': b'a' * 1024 + b'\x00',
    'early_nul': b'a' * 1023 + b'\x00',
    'utf8': '\u00e9\u00e8 \u2603\n'.encode(),
    'latin1': '\u00e9\u00f3\u00f1\u00e5'.encode('latin1'),
    'plist_bin': b'bplist00\xd1\x01\x02_\x10\x0fLast Login Name',
    'plist_txt': b'<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"/>\n',
    'crlf': b'#!/bin/bash\r\necho\r\n',
    'php': b'#!/usr/bin/php8.2\n<?php\n',
    'awk': b'#!/usr/bin/awk -f\n',
    'zsh': b'#!/bin/zsh\n',
}


def path_cases():
    """(case id, file name, content key, mode) regular files created on disk."""
    cases = []
    names = ['noext', 'script', 'x.py', 'x.unknownext', 'f.plist', 'Makefile', 'README',
             '.bashrc', 'x.JPG', 'x.ppm', 'x.sh', 'Dockerfile.dev', 'bin.dat', 'x.']
    for cname in PATH_CONTENTS:
        for mode in (0o644, 0o755):
            for name in names:
                cases.append((f'{cname}-{oct(mode)[2:]}-{name}', name, cname, mode))
    for mode in (0o700, 0o744, 0o711, 0o701, 0o070, 0o007, 0o600):
        cases.append((f'mode-{oct(mode)[2:]}', 'noext', 'zsh', mode))
    return cases


def gen_paths(identify):
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        for case_id, name, cname, mode in path_cases():
            content = PATH_CONTENTS[cname]
            d = os.path.join(tmp, case_id)
            os.mkdir(d)
            p = os.path.join(d, name) if name != 'x.' else os.path.join(d, 'x.')
            with open(p, 'wb') as f:
                f.write(content)
            os.chmod(p, mode)
            try:
                tags = sorted(identify.tags_from_path(p))
            except (OSError, ValueError) as e:
                tags = {'error': type(e).__name__}
            try:
                shebang = list(identify.parse_shebang_from_file(p))
            except (OSError, ValueError) as e:
                shebang = {'error': type(e).__name__}
            readable = bool(mode & stat.S_IRUSR)
            file_is_text = identify.file_is_text(p) if readable else None
            out.append({
                'name': name, 'content': cname, 'mode': mode,
                'tags': tags, 'shebang': shebang, 'file_is_text': file_is_text,
            })
        # special entries
        specials = []
        d = os.path.join(tmp, 'specials')
        os.mkdir(d)
        os.mkdir(os.path.join(d, 'adir'))
        os.mkdir(os.path.join(d, 'adir.py'))
        with open(os.path.join(d, 'target.py'), 'w') as f:
            f.write('x')
        os.symlink('target.py', os.path.join(d, 'link.py'))
        os.symlink('missing', os.path.join(d, 'broken'))
        for name in ('adir', 'adir.py', 'link.py', 'broken', 'missing.py'):
            p = os.path.join(d, name)
            try:
                tags = sorted(identify.tags_from_path(p))
            except ValueError as e:
                tags = {'error': 'ValueError', 'message': str(e).replace(p, '{path}')}
            specials.append([name, tags])
    return out, specials


def detect_version(root: str) -> str:
    """identify's version from `setup.cfg` or an unpacked wheel's METADATA."""
    cfg = configparser.ConfigParser()
    cfg.read(os.path.join(root, 'setup.cfg'))
    version = cfg.get('metadata', 'version', fallback=None)
    if version:
        return version
    for entry in sorted(os.listdir(root)):
        if entry.startswith('identify-') and entry.endswith('.dist-info'):
            with open(os.path.join(root, entry, 'METADATA'), encoding='utf-8') as f:
                for line in f:
                    if line.startswith('Version:'):
                        return line.split(':', 1)[1].strip()
    return 'unknown'


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--identify', required=True)
    ap.add_argument('--out', required=True)
    args = ap.parse_args()
    root = os.path.abspath(args.identify)
    sys.path.insert(0, root)
    from identify import extensions, identify, interpreters  # noqa: E402

    version = detect_version(root)

    rng = random.Random(SEED)
    paths, specials = gen_paths(identify)
    data = {
        'identify_version': version,
        'python_version': sys.version.split()[0],
        'all_tags': sorted(identify.ALL_TAGS),
        'tables': {
            'EXTENSIONS': {k: sorted(v) for k, v in extensions.EXTENSIONS.items()},
            'EXTENSIONS_NEED_BINARY_CHECK': {
                k: sorted(v) for k, v in extensions.EXTENSIONS_NEED_BINARY_CHECK.items()
            },
            'NAMES': {k: sorted(v) for k, v in extensions.NAMES.items()},
            'INTERPRETERS': {k: sorted(v) for k, v in interpreters.INTERPRETERS.items()},
        },
        'tags_from_filename': gen_filenames(rng, identify, extensions),
        'tags_from_interpreter': gen_interpreters(rng, identify, interpreters),
        'parse_shebang': gen_shebangs(rng, identify),
        'shlex_split': gen_shlex(rng),
        'is_text': gen_is_text(rng, identify),
        'path_contents': {k: hexb(v) for k, v in PATH_CONTENTS.items()},
        'tags_from_path': paths,
        'tags_from_path_special': specials,
    }
    with open(args.out, 'w', encoding='utf-8') as f:
        json.dump(data, f, ensure_ascii=True, indent=0, sort_keys=True)
        f.write('\n')
    counts = {k: len(v) for k, v in data.items() if isinstance(v, (list, dict))}
    print(f'wrote {args.out}: identify {version}, {counts}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
