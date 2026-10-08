"""Generate golden vectors for snp's `pycompat` module.

Run with an isolated interpreter, from a directory that contains no .py files:

    python3 -I gen_pycompat_vectors.py OUT.json FIXTURE_DIR [--legacy] [--delta-against BASE.json]

Only the standard library is used. No pre-commit code is imported or executed;
the composite vectors re-state the few pre-commit expressions (cited by
file:line in the spec) inline.

`cargo xtask pycompat-vectors` (WP-11) drives this script: under CPython 3.12.3 it
writes crates/snp-pycompat/tests/vectors/pycompat-vectors.cpython312.json; under
CPython 3.13.14 with `--delta-against` that file it writes
pycompat-delta-cpython313.json, which holds only the entries whose 3.13 result
differs: `{"meta": …, "delta": {section: {index-or-key: 3.13 value}}}`, where the
key is a list index (decimal string) or an object key of the base section, or
`"*"` for a section whose shape changed (replaced as a whole).

Provenance (ADR-053): the 59 sections up to `decimal_zeros` are the recovered
original generator, verbatim except for one sample of the `text_read` section
(`b'a\\xc2\\x85b...'`, the bytes the rendered 15 §A4.4 table shows; the recovered copy
had lost the `\\xc2` and crashed in `read_universal`). With `--legacy` the WP-11
extension sections are omitted and CPython 3.12.3 reproduces the original file
byte for byte (sha256 8c642b5fdf8c2d9b86786aa8bbcefc81cd97ed22eb77396e5095261cd7710004,
15 §A1); the extension sections (`EXT_SECTIONS`) follow them in the full file.
"""
from __future__ import annotations

import io
import json
import ntpath
import os
import platform
import posixpath
import re
import shlex
import sys
import textwrap
import unicodedata


def _parse_args(argv):
    pos, legacy, delta_base = [], False, None
    it = iter(argv)
    for a in it:
        if a == '--legacy':
            legacy = True
        elif a == '--delta-against':
            delta_base = next(it, None)
            if delta_base is None:
                sys.exit('--delta-against needs a file')
        elif a.startswith('--'):
            sys.exit(f'unknown option {a}')
        else:
            pos.append(a)
    if len(pos) != 2:
        sys.exit(__doc__)
    return pos[0], pos[1], legacy, delta_base


OUT, FIXTURE, LEGACY, DELTA_BASE = _parse_args(sys.argv[1:])

V: dict = {
    'meta': {
        'generator': 'scripts/gen_pycompat_vectors.py',
        'python': platform.python_version(),
        'implementation': platform.python_implementation(),
        'unicode': unicodedata.unidata_version,
        'platform': sys.platform,
        'notes': (
            'bytes are hex strings in *_hex fields; strings that would need '
            'lone surrogates are given as code point lists in *_cps fields; '
            'errors are {"error": "<ExcType>", "msg": "<str(exc)>"}'
        ),
    },
}


def err(e: BaseException) -> dict:
    return {'error': type(e).__name__, 'msg': str(e)}


def call(f, *a, **k):
    try:
        return {'ok': f(*a, **k)}
    except Exception as e:  # noqa: BLE001
        return err(e)


def ranges(pred) -> list[list[int]]:
    out: list[list[int]] = []
    start = None
    for cp in range(0x110000):
        if pred(cp):
            if start is None:
                start = cp
        elif start is not None:
            out.append([start, cp - 1])
            start = None
    if start is not None:
        out.append([start, 0x10FFFF])
    return out


# ---------------------------------------------------------------- char sets
WS_RE = re.compile(r'\s')
WSB_RE = re.compile(rb'\s')
RUST_WHITE_SPACE = [  # Unicode PropList White_Space (stable since 6.3)
    [0x09, 0x0D], [0x20, 0x20], [0x85, 0x85], [0xA0, 0xA0], [0x1680, 0x1680],
    [0x2000, 0x200A], [0x2028, 0x2029], [0x202F, 0x202F], [0x205F, 0x205F],
    [0x3000, 0x3000],
]


def in_ranges(cp, rs):
    return any(a <= cp <= b for a, b in rs)


V['sets'] = {
    'str_isspace': ranges(lambda cp: chr(cp).isspace()),
    're_str_backslash_s': ranges(lambda cp: WS_RE.match(chr(cp)) is not None),
    'str_splitlines_boundary': ranges(
        lambda cp: len(('a' + chr(cp) + 'b').splitlines()) == 2,
    ),
    'bytes_isspace': [b for b in range(256) if bytes([b]).isspace()],
    're_bytes_backslash_s': [
        b for b in range(256) if WSB_RE.match(bytes([b])) is not None
    ],
    'bytes_splitlines_boundary': [
        b for b in range(256) if len((b'a' + bytes([b]) + b'b').splitlines()) == 2
    ],
    'rust_white_space_property': RUST_WHITE_SPACE,
    'str_isspace_minus_rust_white_space': ranges(
        lambda cp: chr(cp).isspace() and not in_ranges(cp, RUST_WHITE_SPACE),
    ),
    'rust_white_space_minus_str_isspace': ranges(
        lambda cp: in_ranges(cp, RUST_WHITE_SPACE) and not chr(cp).isspace(),
    ),
    # repr() escapes non-ASCII code points for which this is False
    'str_isprintable_false': ranges(lambda cp: not chr(cp).isprintable()),
}

# ---------------------------------------------------------------- splitlines
SL_STR = [
    '', '\n', '\n\n', 'a', 'a\n', 'a\nb', 'a\n\nb', 'a\r\nb', 'a\rb', 'a\n\rb',
    'a\r\r\nb', '\r', '\r\n', '\r\r', 'a\r', 'a\r\n', '\n\r\n', 'a\x0bb',
    'a\x0cb', 'a\x1cb', 'a\x1db', 'a\x1eb', 'a\x1fb', 'a\x85b', 'a\u2028b',
    'a\u2029b', 'a\x00b', 'a\tb', 'a\u0085', '\u2028', 'x\r\n\r\ny',
    'refs/heads/a\u2028b 1111 refs/heads/x 0000\n', 'a\x0b\x0c\x1c\x1d\x1e\x85',
    '  rev: v1\r\n  rev: v2\n  rev: v3', 'no newline at end',
]
V['str_splitlines'] = [
    {'in': s, 'keepends': k, 'out': s.splitlines(k)}
    for s in SL_STR for k in (False, True)
]
SL_BYTES = [
    b'', b'\n', b'a\nb', b'a\r\nb', b'a\rb', b'a\n\rb', b'a\r\r\nb', b'a\x0bb',
    b'a\x0cb', b'a\x1cb', b'a\x85b', b'a\xc2\x85b', b'a\xe2\x80\xa8b',
    b'#\tfoo.py\r\n#\tbar.py\n\tbaz.py', b'a\r', b'\r\n\r\n', b'x\n',
]
V['bytes_splitlines'] = [
    {'in_hex': b.hex(), 'keepends': k, 'out_hex': [x.hex() for x in b.splitlines(k)]}
    for b in SL_BYTES for k in (False, True)
]

# ------------------------------------------------------------- split / strip
SPLIT_STR = [
    '', ' ', '  ', 'a', ' a ', 'a  b', 'a\tb\nc', 'a\xa0b', 'a\u3000b',
    'a\x1fb', 'a\x1cb', 'a\u200bb', 'a\x85b', 'a\u180eb', 'a\ufeffb',
    'a\u2007b', 'a\u202fb', '\t a b \n',
]
V['str_split_none'] = [{'in': s, 'out': s.split()} for s in SPLIT_STR]
V['str_split_none_maxsplit'] = [
    {'in': s, 'maxsplit': m, 'out': s.split(None, m)}
    for s in ['a b c', '  a b  c  ', 'a  b  c  d  e', ' x ', '']
    for m in (0, 1, 2)
]
RSPLIT_IN = [
    'refs/heads/main 1111111111111111111111111111111111111111 '
    'refs/heads/main 2222222222222222222222222222222222222222',
    'HEAD@{1 day ago} 1111 refs/heads/x 0000',
    'a b c d', 'a b c', 'a b', 'a', '', '   ', 'a b c d e f',
    'a  b  c  d  e', '  a b c d', '  a b c d e', 'a b c d  ', '\ta\tb\tc\td\t',
    'l\xa0ls r rs', 'l ls r\xa0x rs', 'l ls r\x1fx rs', 'l ls r\u3000x rs',
    'l ls r\x85x rs', 'l ls r\u200bx rs', 'a\x0bb c d',
]
V['str_rsplit_none_maxsplit'] = [
    {'in': s, 'maxsplit': m, 'out': s.rsplit(None, m)}
    for s in RSPLIT_IN for m in (3, -1)
]
V['str_split_sep'] = [
    {'in': s, 'sep': sep, 'out': s.split(sep)}
    for s, sep in [
        ('', ','), (',', ','), ('a,b', ','), (' a , b ', ','), ('a,,b', ','),
        (',a,', ','), ('a', ','), ('', ':'), ('/usr/bin', ':'),
        ('/a::/b:', ':'), (':', ':'), ('.COM;.EXE;.BAT', ';'), (';.EXE', ';'),
        ('a\x00b\x00', '\x00'), ('1.2.3', '.'), ('1..2', '.'),
        ('v1.0', '.'), ('', '.'), ('rev: v1', ':'),
    ]
]
STRIP_STR = [
    '', ' ', 'a', '  a  ', '\ta\n', '\r\n', 'a\xa0', '\xa0a', 'a\x1f', 'a\x1c',
    'a\x85', '\u3000a\u3000', 'a\u200b', 'a\ufeff', '\x0b\x0ca\x0c\x0b',
    'a\x00', '\x00a\x00\x00', 'a\u2028', 'a\u180e',
]
V['str_strip_none'] = [
    {'in': s, 'strip': s.strip(), 'lstrip': s.lstrip(), 'rstrip': s.rstrip()}
    for s in STRIP_STR
]
V['str_strip_chars'] = [
    {'in': s, 'chars': c, 'strip': s.strip(c), 'lstrip': s.lstrip(c),
     'rstrip': s.rstrip(c)}
    for s, c in [
        ('\x00a\x00b\x00\x00', '\x00'), ('a\n\n', '\n'), ('a\r\n', '\n'),
        ('\n', '\n'), ('', '\n'), ('##a#', '#'), ('ab', ''),
    ]
]
STRIP_BYTES = [
    b'', b' ', b'  a  ', b'\ta\n', b'a\x0b', b'a\x0c', b'a\x1c', b'a\x1f',
    b'a\x85', b'a\xa0', b'a\xc2\xa0', b'\r\n', b'a\x00',
]
V['bytes_strip_none'] = [
    {'in_hex': b.hex(), 'strip_hex': b.strip().hex(),
     'lstrip_hex': b.lstrip().hex(), 'rstrip_hex': b.rstrip().hex()}
    for b in STRIP_BYTES
]
V['bytes_strip_chars'] = [
    {'in_hex': b.hex(), 'chars_hex': c.hex(), 'strip_hex': b.strip(c).hex(),
     'lstrip_hex': b.lstrip(c).hex(), 'rstrip_hex': b.rstrip(c).hex()}
    for b, c in [
        (b'##\tfoo', b'#'), (b'#\t#foo', b'#'), (b'foo\r\n', b'\r\n'),
        (b'foo\r\r\n', b'\r\n'), (b'foo\n\r', b'\r\n'), (b'foo\r', b'\r\n'),
        (b'\r\n', b'\r\n'), (b'foo \r\n', b'\r\n'), (b'foo\n', b'\n'),
    ]
]

# ------------------------------------------------- pre-commit composites
# git.py:19-24
def zsplit(s: str) -> list[str]:
    s = s.strip('\0')
    if s:
        return s.split('\0')
    else:
        return []


V['zsplit'] = [
    {'in': s, 'out': zsplit(s)}
    for s in ['', '\0', '\0\0', 'a', 'a\0', 'a\0b\0', '\0a\0', 'a\0\0b\0',
              ' a \0 b\0', 'a b\0']
]


# run.py:129-131
def get_skips(s: str) -> list[str]:
    return sorted({skip.strip() for skip in s.split(',') if skip.strip()})


V['skip_env'] = [
    {'in': s, 'out_sorted_set': get_skips(s)}
    for s in ['', ' ', ',', 'a', 'a,b', ' a , b ', 'a,,b', ',a,', 'a,a',
              'a b,c', 'a\xa0,b', '\xa0', 'a\x1f', 'a\x1c,b', 'a\u200b',
              'a\tb', 'flake8, mypy ,\tblack\n']
]


# hook_impl.py:120-130 (parsing part only)
def pre_push_parse(stdin: bytes) -> dict:
    try:
        text = stdin.decode()
    except UnicodeDecodeError as e:
        return {'decode': err(e)}
    out = []
    for line in text.splitlines():
        parts = line.rsplit(maxsplit=3)
        try:
            local_branch, local_sha, remote_branch, remote_sha = parts
        except ValueError as e:
            out.append({'line': line, **err(e)})
            break  # pre-commit crashes here (exit 3)
        out.append({'line': line, 'fields': [local_branch, local_sha,
                                             remote_branch, remote_sha]})
    return {'lines': out}


Z = '0' * 40
O1 = '1' * 40
O2 = '2' * 40
PP = [
    f'refs/heads/main {O1} refs/heads/main {O2}\n'.encode(),
    f'refs/heads/a {O1} refs/heads/a {Z}\nrefs/heads/b {O2} refs/heads/b {Z}\n'.encode(),
    f'(delete) {Z} refs/heads/gone {O2}\n'.encode(),
    f'HEAD@{{1 day ago}} {O1} refs/heads/x {Z}\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main {O2}\r\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main {O2}'.encode(),
    b'',
    b'\n',
    f'refs/heads/main {O1} refs/heads/main {O2}\n\n'.encode(),
    f'refs/heads/main {O1} refs/heads/main\n'.encode(),
    f'refs/heads/caf\xe9 {O1} refs/heads/x {Z}\n'.encode(),
    f'refs/heads/caf\xe9 {O1} refs/heads/x {Z}\n'.encode('latin-1'),
    f'refs/heads/a\u2028b {O1} refs/heads/a\u2028b {Z}\n'.encode(),
    f'refs/heads/a\x85b {O1} refs/heads/a\x85b {Z}\n'.encode(),
    f'refs/heads/a\xa0b {O1} refs/heads/a\xa0b {Z}\n'.encode(),
    f'refs/heads/a\u3000b {O1} refs/heads/x {Z}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/a\x1fb {Z}\n'.encode(),
    f'  refs/heads/x {O1} refs/heads/x {Z}  \n'.encode(),
    f'refs/heads/x\t{O1}\trefs/heads/x\t{Z}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/x {Z}\x0crefs/heads/y {O2} refs/heads/y {Z}\n'.encode(),
    f'refs/heads/x {O1} refs/heads/x {Z}\rrefs/heads/y {O2} refs/heads/y {Z}\n'.encode(),
]
V['pre_push_parse'] = [{'in_hex': b.hex(), 'out': pre_push_parse(b)} for b in PP]


# git.py:104-111
def merge_conflicts(b: bytes) -> dict:
    try:
        return {'ok': [
            line.lstrip(b'#').strip().decode()
            for line in b.splitlines()
            if line.startswith((b'\t', b'#\t'))
        ]}
    except UnicodeDecodeError as e:
        return err(e)


MM = [
    b"Merge branch 'foo'\n\n# Conflicts:\n#\tconflict_file\n#\tother\n",
    b"Merge branch 'foo'\n\nConflicts:\n\tconflict_file\n",
    b'Merge\r\n\r\n# Conflicts:\r\n#\ta.txt\r\n#\tb c.txt \r\n',
    b'#\t  spaced  \n##\tdouble\n# \tnot\n\t\x0bvt\x0b\n#\tx\x0cy\n',
    b'#\tcaf\xc3\xa9\n',
    b'#\tcaf\xe9\n',
    b'#\ta\x85b\n#\tc\x1cd\n',
    b'#\tfile\rwith-cr\n',
    b'',
]
V['merge_msg_conflicts'] = [{'in_hex': b.hex(), 'out': merge_conflicts(b)} for b in MM]

# autoupdate.py:116
REV_LINE_RE = re.compile(r'^(\s+)rev:(\s*)([\'"]?)([^\s#]+)(.*)(\r?\n)$')
RL = [
    '    rev: v1.0.0\n', '    rev: v1.0.0\r\n', '    rev: v1.0.0',
    "    rev: 'v1.0.0'\n", '    rev: "v1.0.0"\n', '-   rev: v1\n',
    '  - rev: v1\n', 'rev: v1\n', '    rev:v1\n', '    rev:\tv1\n',
    '    rev: v1  # frozen: v1.0.0\n', '    rev: abc  # frozen: v1\r\n',
    '    rev: v1 # comment\n', '    rev: #x\n', '    rev: \n',
    '\xa0\xa0rev: v1\n', '\x1f rev: v1\n', '    rev:\xa0v1\n',
    '    rev: v1\xa0\n', '    rev: v1\x1f\n', '    rev: v1\u3000x\n',
    '    rev: v1\r\r\n', '    rev: v1\n\n', '    rev: v1\x85',
    '    rev: v1\u2028', '    rev: v\u00e9\n', '    rev: \'v1\'  \n',
    '    rev: "v1\'\n', '\trev: v1\n', '    revision: v1\n',
    '    rev: [v1]\n', '    rev: v1\r',
]


def rev_match(line: str):
    m = REV_LINE_RE.match(line)
    return None if m is None else list(m.groups())


V['rev_line_re'] = [{'in': s, 'groups': rev_match(s)} for s in RL]


# autoupdate.py:146-156 -- the line assembly (yaml_dump result is an input)
def rev_rewrite(line: str, new_rev: str, frozen):
    match = REV_LINE_RE.match(line)
    assert match is not None
    if frozen is not None:
        comment = f'  # frozen: {frozen}'
    elif match[5].strip().startswith('# frozen:'):
        comment = ''
    else:
        comment = match[5]
    return f'{match[1]}rev:{match[2]}{new_rev}{comment}{match[6]}'


V['rev_line_rewrite'] = [
    {'line': ln, 'new_rev': nr, 'frozen': fz, 'out': rev_rewrite(ln, nr, fz)}
    for ln, nr, fz in [
        ('    rev: v1.0.0\n', 'v2.0.0', None),
        ('    rev: v1.0.0\r\n', 'v2.0.0', None),
        ('    rev: v1.0.0\r\n', 'a' * 40, 'v2.0.0'),
        ('    rev: ' + 'a' * 40 + '  # frozen: v1\r\n', 'v2', None),
        ('    rev: ' + 'a' * 40 + '  # frozen: v1\n', 'v2', None),
        ('    rev: v1  # keep me\n', 'v2', None),
        ('    rev: v1  # keep me\r\n', 'b' * 40, 'v2'),
        ("    rev: 'v1'\n", "'v2'", None),
        ('    rev: v1 #frozen: x\n', 'v2', None),
        ('    rev: v1\t# frozen: x\n', 'v2', None),
        ('    rev: v1\xa0# frozen: x\n', 'v2', None),
    ]
]


# autoupdate.py:120-130: which line indexes are rev lines
def rev_line_idxs(text: str):
    lines = text.splitlines(True)
    return {'lines': lines,
            'idxs': [i for i, line in enumerate(lines) if REV_LINE_RE.match(line)]}


V['autoupdate_original_lines'] = [
    {'in': t, 'out': rev_line_idxs(t)}
    for t in [
        'repos:\n-   repo: r\n    rev: v1\n    hooks:\n    -   id: x\n',
        'repos:\r\n-   repo: r\r\n    rev: v1\r\n    hooks: []\r\n',
        'repos:\n-   repo: r\n    rev: v1',
        'repos:\n-   repo: r\n    rev: v1\x0c\n',
        'repos:\n-   repo: r\n    rev: v1\x85    rev: v2\n',
        'repos:\n-   repo: r\n    rev: v1\u2028\n',
        'repos:\r-   repo: r\r    rev: v1\r',
        'repos:\n-   {repo: r, rev: v1, hooks: []}\n',
        'repos:\n- repo: r\n  rev: v1 # c\n  hooks: []\n',
    ]
]


# migrate_config.py:21-46 (header split) + textwrap.indent
def is_header_line(line: str) -> bool:
    return line.startswith(('#', '---')) or not line.strip()


def header_split(contents: str):
    lines = contents.splitlines(True)
    i = 0
    while i < len(lines) and is_header_line(lines[i]):
        i += 1
    return {'header': ''.join(lines[:i]), 'rest': ''.join(lines[i:]),
            'indented_rest': textwrap.indent(''.join(lines[i:]), ' ' * 4)}


V['migrate_header_split'] = [
    {'in': t, 'out': header_split(t)}
    for t in [
        '-   repo: r\n    hooks: []\n',
        '# comment\n---\n\n-   repo: r\n',
        '#c\n   \n\t\n-   repo: r\n',
        '\xa0\n-   repo: r\n',
        '\x1f\n-   repo: r\n',
        '\u200b\n-   repo: r\n',
        '#c\x0c-   repo: r\n',
        '# only comments\n',
        '',
        '- a\n\n- b\n   \n- c',
        '- a\n\x0b- b\n',
    ]
]
V['textwrap_indent'] = [
    {'in': t, 'prefix': '    ', 'out': textwrap.indent(t, '    ')}
    for t in ['', 'a', 'a\nb\n', 'a\n\nb', 'a\n  \nb', 'a\r\nb', 'a\x0cb',
              'a\n\xa0\nb', 'a\n\x1f\nb', 'a\u2028b', ' \n', '\n']
]

# ------------------------------------------------- text I/O semantics


def read_universal(b: bytes) -> str:
    return io.TextIOWrapper(io.BytesIO(b), encoding='utf-8', newline=None).read()


def read_newline_empty(b: bytes) -> str:
    return io.TextIOWrapper(io.BytesIO(b), encoding='utf-8', newline='').read()


def write_translate(s: str, newline) -> bytes:
    buf = io.BytesIO()
    w = io.TextIOWrapper(buf, encoding='utf-8', newline=newline)
    w.write(s)
    w.flush()
    return buf.getvalue()


TIO = [b'a\r\nb\rc\nd', b'\r', b'\r\r\n', b'\n\r', b'a\xc2\x85b\xe2\x80\xa8c',
       b'\xef\xbb\xbfa\r\n', b'a\r\n', b'', b'x\x0cy\x0bz']
V['text_read'] = [
    {'in_hex': b.hex(), 'newline_none': read_universal(b),
     'newline_empty': read_newline_empty(b)}
    for b in TIO
]
V['text_write'] = [
    {'in': s, 'posix_hex': write_translate(s, None).hex(),
     'windows_hex': write_translate(s, '\r\n').hex(),
     'newline_empty_hex': write_translate(s, '').hex()}
    for s in ['a\nb\n', 'a\r\nb', 'a\rb', '', 'no-eol']
]

# ------------------------------------------------- UTF-8 decode errors
DEC = [b'', b'ok', b'\xff', b'\x80', b'a\xc3', b'\xc3\x28', b'\xe2\x82',
       b'\xe2\x28\xa1', b'\xe2\x82\x28', b'\xf0\x9f\x98', b'\xf0\x28\x8c\x28',
       b'\xc0\xaf', b'\xc1\xbf', b'\xe0\x80\xaf', b'\xe0\xa0', b'\xed\xa0\x80',
       b'\xf4\x90\x80\x80', b'\xf5\x80', b'\xf8\x88\x80\x80\x80',
       b'ok\xffok', b'abc\xe2\x82', b'\xe2\x82abc', b'\xf0\x9f\x98\x80\xff',
       b'caf\xe9.txt\x00', b'\xef\xbb\xbfbom']
V['utf8_decode_strict'] = [
    {'in_hex': b.hex(), 'out': call(lambda b=b: b.decode())} for b in DEC
]
V['utf8_decode_backslashreplace'] = [
    {'in_hex': b.hex(), 'out': b.decode(errors='backslashreplace')}
    for b in [b'git version 2.45.0\n', b'git version \xff\xfe\n',
              b'\xe2\x82abc', b'\xf0\x9f\x98', b'\xc3\xa9\x80']
]

# ------------------------------------------------- repr


def tag(v):
    if v is None:
        return {'t': 'none'}
    if isinstance(v, bool):
        return {'t': 'bool', 'v': v}
    if isinstance(v, int):
        return {'t': 'int', 'v': v}
    if isinstance(v, float):
        return {'t': 'float', 'v': repr(v)}
    if isinstance(v, str):
        try:
            v.encode('utf-8')
            return {'t': 'str', 'v': v}
        except UnicodeEncodeError:
            return {'t': 'str', 'v_cps': [ord(c) for c in v]}
    if isinstance(v, tuple):
        return {'t': 'tuple', 'v': [tag(x) for x in v]}
    if isinstance(v, list):
        return {'t': 'list', 'v': [tag(x) for x in v]}
    if isinstance(v, dict):
        return {'t': 'dict', 'v': [[tag(k), tag(x)] for k, x in v.items()]}
    raise TypeError(type(v))


REPR_STR = [
    '', 'a', "it's", 'say "hi"', 'both \'"', 'back\\slash', '^\\.py(',
    'tab\t', 'nl\n', 'cr\r', 'nul\x00', 'bel\x07', '\x1b[31m', 'del\x7f',
    '\x80', '\x9f', '\xa0', '\xad', '\xe9', 'caf\xe9', '\xdf', '\u0378',
    '\u200b', '\u200e', '\u2028', '\u2029', '\u3000', '\ufeff', '\ufffd',
    '\uffff', '\ue000', '\U0001f600', '\U0001fae9', '\U000e0001',
    '\U0010ffff', '\U000f0000', 'C:\\Users\\a b\\python.exe',
    '/tmp/it\'s "x"', '\'', '"', '\\', 'a\x0bb\x0cc', 'x\x85y',
    '\U0001f1fa\U0001f1f8', 'e\u0301', '\u0600', '\u061c', '\u1680',
    '\U00013430', '\U0001d173', 'sub/', 'hook-impl',
    '\udcff', '/tmp/\udcff\udc80',
]
REPR_OTHER = [
    [], ['a'], ['a', 'b'], ["it's"], ['a', ['b', ('c',)]], [1, 'a'],
    [True, False, None], [1.0, 1e16, 1.5e-07, -0.0],
    (), ('a',), ('a', 'b'), ('/usr/bin/git', 'write-tree'),
    ('git', 'diff-index', '--ignore-submodules', '--binary', '--exit-code',
     '--no-color', '--no-ext-diff', 'deadbeef', '--'),
    ((),), ([],), {}, {'a': 1}, {'k': ['v']}, 0, -1, 10**20, True, None,
    1.0, 0.1, 1e16, 1e-05, float('inf'), float('nan'),
    ['--hook-type=pre-commit', 'x y'], ['pyyaml==6.0', "it's"],
]
V['repr'] = [{'in': tag(v), 'out': repr(v)} for v in REPR_STR + REPR_OTHER]
def _fstring_eq(prefix):
    return f'{prefix=}'


V['fstring_eq'] = [
    {'name': 'prefix', 'in': tag(v), 'out': _fstring_eq(v)}
    for v in ['sub/', "it's/", 'a\\b/', 'tab\t/']
]


def oserror_str(*args):
    return str(OSError(*args))


V['oserror_str'] = [
    {'args': list(a), 'out': oserror_str(*a)}
    for a in [
        (2, 'No such file or directory', './x.sh'),
        (13, 'Permission denied', '/repo/hook.sh'),
        (8, 'Exec format error', './noshebang.sh'),
        (7, 'Argument list too long', '/usr/bin/git'),
        (2, 'No such file or directory'),
        (2, 'No such file or directory', "it's"),
        (2, 'No such file or directory', 'a', None, 'b'),
        ('just a message',),
        (),
    ]
]
V['errno_strerror_glibc'] = {
    str(n): os.strerror(n) for n in (1, 2, 7, 8, 13, 20, 21, 26, 36, 40)
}


def int_err(s):
    try:
        return {'ok': tuple(int(p) for p in s.split('.'))}
    except ValueError as e:
        return err(e)


V['parse_version'] = [  # clientlib.py:50-52
    {'in': s, 'out': int_err(s)}
    for s in ['1.2.3', '4.6.2', '1', '1. 2', ' 1.2 ', '1.x', '', '1..2',
              '1_0.2', '\u0663.1', '1.2-dev', "it's", '1.2\n']
]

# ------------------------------------------------- shlex
Q = ['', 'a', 'a b', "it's", '\xe9', 'a$b', '-x', '=x', '~', '*', '!',
     'a\nb', '@%+=:,./-_', 'C:\\Python\\python.exe',
     '/usr/bin/python3', "'", '"', 'a"b', '\t', '--config=.pre-commit-config.yaml',
     '--config=my config.yaml', '/home/u/.cache/pre-commit/repoabc/py_env-python3',
     'x\x00y', 'A-Z_a-z0-9', '\u00e9t\u00e9', '#']
V['shlex_quote'] = [{'in': s, 'out': shlex.quote(s)} for s in Q]
V['shlex_join'] = [
    {'in': a, 'out': shlex.join(a)}
    for a in [
        [], [''], ['hook-impl', '--config=.pre-commit-config.yaml',
                   '--hook-type=pre-commit'],
        ['hook-impl', '--config=a b.yaml', '--hook-type=pre-push',
         '--skip-on-missing-config'],
        ['a b', "c'd", '', '$x'],
    ]
]
SPLIT = [
    '', ' ', 'a', 'a b', '  a   b  ', 'a\tb\nc', "'a b'", '"a b"', "a'b'c",
    'a"b"c', '"a\\"b"', '"a\\\\b"', '"a\\$b"', '"a\\b"', '"a\\`b"',
    '"a\\\nb"', "'a\\b'", 'a\\ b', 'a\\\\b', 'a\\$b', 'a\\\nb', '\\',
    '"unterminated', "'unterminated", 'a # b', 'a#b', '#a', "''", '""',
    "'' x", 'a\'\'b', 'pre-commit hazmat cd sub cmd', '\xa0a', 'a\xa0b',
    'a\x1fb', 'a\x0bb', 'a\x0cb', 'a\rb', 'é ü', '\u3000a',
    'python -c "import sys; print(sys.argv)"', '~/bin/x', 'a;b', 'a|b',
    'a&&b', '$HOME', '"$HOME"', 'a\\', '"a\\', "it's", "'it'\\''s'",
]
V['shlex_split'] = [{'in': s, 'out': call(shlex.split, s)} for s in SPLIT]

# ------------------------------------------------- posixpath
POSIX_CWD = '/home/u/repo/sub'
NT_CWD = 'C:\\Users\\u\\repo\\sub'
_real_getcwd = os.getcwd


def with_cwd(cwd, f, *a):
    os.getcwd = lambda: cwd  # type: ignore[assignment]
    try:
        return call(f, *a)
    finally:
        os.getcwd = _real_getcwd  # type: ignore[assignment]


PNORM = ['', '.', '..', '/', '//', '///', '////a', '//a//b', '/..', '/../a',
         '../..', 'a/..', 'a/../..', './a/./b/.', 'a//b', 'a/b/', 'a/b/../../..',
         '/a/b/../../../c', '~', '~/x/../y', 'a/./../b', '.a/..b/...',
         '/./', 'a\\b', 'C:\\x', '/a/b/./c/', '..//./..']
V['posixpath_normpath'] = [{'in': p, 'out': posixpath.normpath(p)} for p in PNORM]
V['posixpath_abspath'] = [
    {'cwd': POSIX_CWD, 'in': p, 'out': with_cwd(POSIX_CWD, posixpath.abspath, p)}
    for p in ['', '.', 'x', '../x', '../../../../../x', '/abs/./x/..', '//x',
              'a/b/', '~']
]
V['posixpath_join'] = [
    {'in': list(a), 'out': posixpath.join(*a)}
    for a in [('a', 'b'), ('a/', 'b'), ('a', '/b'), ('a', ''), ('', 'b'),
              ('', ''), ('/', 'x'), ('a', 'b', '/c', 'd'), ('a//', 'b'),
              ('/r', 'py_env-/usr/bin/python3'), ('/r', 'py_env-3.12')]
]
V['posixpath_relpath'] = [
    {'cwd': POSIX_CWD, 'path': p, 'start': s,
     'out': with_cwd(POSIX_CWD, posixpath.relpath, p, *([] if s is None else [s]))}
    for p, s in [
        ('/home/u/repo/sub/x', None), ('/home/u/repo/x', None),
        ('/home/u/repo', None), ('/home/u/repo/sub', None), ('/', None),
        ('/etc/passwd', '/home/u/repo'), ('/home/u/repo/a/b', '/home/u/repo'),
        ('/home/u/repo', '/home/u/repo'), ('x', None), ('', None),
        ('/home/u/repo/.pre-commit-config.yaml', '/home/u/repo'),
        ('/a//b/./c', '/a/b'), ('//a', '/a'), ('/a', '/a/'),
    ]
]
V['posixpath_commonpath'] = [
    {'in': list(a), 'out': call(posixpath.commonpath, list(a))}
    for a in [('/usr/bin/node', '/home/u'), ('/home/u/.nvm/bin/node', '/home/u'),
              ('/home/u', '/home/u'), ('/home/user2/x', '/home/u'),
              ('bin/node', '/home/u'), ('/', '/home/u'), (), ('a/b', 'a/c'),
              ('/a/./b', '/a/b/c'), ('/a/../b', '/a/b'), ('//a', '/a'),
              ('/home/u/', '/home/u'), ('', ''), ('/x', '')]
]
V['posixpath_parts'] = [
    {'in': p, 'dirname': posixpath.dirname(p), 'basename': posixpath.basename(p),
     'split': list(posixpath.split(p)), 'splitext': list(posixpath.splitext(p)),
     'isabs': posixpath.isabs(p)}
    for p in ['', '/', '//', 'a', '/a', 'a/', '/a/b', 'a/b/', '//a//b', 'a.b.c',
              '.bashrc', '..', 'foo.', '...md', 'a..md', 'dir.d/file', '/a/b.tar.gz',
              'C:\\x\\y.exe', 'app.json', '/x/coursier-app.json']
]


def expanduser_env(env: dict, path: str, mod):
    saved = {k: os.environ.get(k) for k in
             ('HOME', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH', 'USERNAME')}
    try:
        for k in saved:
            os.environ.pop(k, None)
        os.environ.update(env)
        return call(mod.expanduser, path)
    finally:
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


V['posixpath_expanduser'] = [
    {'env': e, 'in': p, 'out': expanduser_env(e, p, posixpath)}
    for e in [{'HOME': '/home/u'}, {'HOME': '/home/u/'}, {'HOME': ''},
              {'HOME': '/'}, {'HOME': '//'}, {'HOME': 'rel/home'}]
    for p in ['~', '~/', '~/.cache', '~/.pre-commit', 'x/~', '', '~~', '~/~']
]

# ------------------------------------------------- ntpath (Windows semantics)
NNORM = ['', '.', '..', '\\', '/', 'C:', 'C:\\', 'c:/', 'C:..', 'C:\\..',
         'C:\\a\\..\\..\\b', 'C:a\\..\\..', 'a/b\\c', 'a//b', '\\\\server\\share',
         '\\\\server\\share\\..\\x', '//server/share/a/../b', '\\\\?\\C:\\a\\..\\b',
         '\\\\.\\pipe\\x', '\\a\\..\\..', 'a\\b\\', 'C:\\a.\\b ', '..\\..\\a',
         '\\\\', '\\\\server', 'C:\\Users\\u\\.cache\\pre-commit\\repo\\node_env-default']
V['ntpath_normpath'] = [{'in': p, 'out': ntpath.normpath(p)} for p in NNORM]
def nt_real_may_differ(*ps):
    for p in ps:
        if p is None:
            continue
        d, r, rest = ntpath.splitroot(p)
        if bool(d) != bool(r) and not d.startswith(('\\\\', '//')):
            return True  # root-relative (\\x) or drive-relative (D:x)
        last = ntpath.normpath(p).rsplit('\\', 1)[-1]
        if last not in ('.', '..') and last.rstrip('. ') != last:
            return True  # GetFullPathNameW trims trailing dots/spaces
    return False


V['ntpath_abspath_fallback'] = [
    {'cwd': NT_CWD, 'in': p, 'out': with_cwd(NT_CWD, ntpath.abspath, p),
     'real_windows_may_differ': nt_real_may_differ(p)}
    for p in ['', '.', 'x', '..\\x', '../x', 'C:\\abs\\x\\..', 'D:\\x', 'D:x',
              '\\x', '/x', '\\\\srv\\share\\x', 'a/b/']
]
V['ntpath_join'] = [
    {'in': list(a), 'out': ntpath.join(*a)}
    for a in [('a', 'b'), ('a\\', 'b'), ('a/', 'b'), ('a', '\\b'), ('C:\\a', 'b'),
              ('C:\\a', 'D:b'), ('C:\\a', 'D:\\b'), ('C:\\a', '\\b'),
              ('c:\\a', 'C:b'), ('C:', 'b'), ('C:', '\\b'), ('a', ''), ('', 'b'),
              ('\\\\srv\\share', 'x'), ('\\\\srv\\share\\', '\\x'),
              ('C:\\r', 'py_env-C:\\Python312\\python.exe'), ('C:\\r', 'node_env-default')]
]
V['ntpath_relpath'] = [
    {'cwd': NT_CWD, 'path': p, 'start': s,
     'out': with_cwd(NT_CWD, ntpath.relpath, p, *([] if s is None else [s])),
     'real_windows_may_differ': nt_real_may_differ(p, s)}
    for p, s in [
        ('C:\\Users\\u\\repo\\sub\\x', None), ('C:\\Users\\u\\repo\\x', None),
        ('c:\\users\\U\\REPO\\x', 'C:\\Users\\u\\repo'), ('D:\\x', None),
        ('D:\\x', 'C:\\Users\\u\\repo'), ('C:/Users/u/repo/a/b', 'C:\\Users\\u\\repo'),
        ('\\\\srv\\share\\x', 'C:\\'), ('x', None), ('', None),
        ('C:\\Users\\u\\repo', 'C:\\Users\\u\\repo'),
    ]
]
V['ntpath_commonpath'] = [
    {'in': list(a), 'out': call(ntpath.commonpath, list(a))}
    for a in [('C:\\Program Files\\nodejs\\node.exe', 'C:\\Users\\u'),
              ('C:\\Users\\u\\AppData\\x.exe', 'C:\\Users\\u'),
              ('c:\\users\\U\\x.exe', 'C:\\Users\\u'),
              ('D:\\tools\\node.exe', 'C:\\Users\\u'),
              ('bin\\node.exe', 'C:\\Users\\u'), ('C:/a/b', 'C:\\a\\c'),
              ('\\a\\b', 'C:\\a'), ('\\\\srv\\share\\a', '\\\\SRV\\Share\\b'), ()]
]
V['ntpath_parts'] = [
    {'in': p, 'dirname': ntpath.dirname(p), 'basename': ntpath.basename(p),
     'split': list(ntpath.split(p)), 'splitext': list(ntpath.splitext(p)),
     'splitdrive': list(ntpath.splitdrive(p)), 'isabs': ntpath.isabs(p),
     'normcase': ntpath.normcase(p)}
    for p in ['', 'C:', 'C:\\', 'C:x', 'C:\\a\\b.exe', 'a/b\\c.txt', '\\a', '/a',
              '\\\\srv\\share', '\\\\srv\\share\\x\\y', '//srv/share/x',
              'C:\\a\\', 'x.', 'C:\\Python312\\python.EXE', '\\\\?\\C:\\x']
]
V['ntpath_expanduser'] = [
    {'env': e, 'in': p, 'out': expanduser_env(e, p, ntpath)}
    for e in [{'USERPROFILE': 'C:\\Users\\u'}, {'USERPROFILE': ''},
              {'HOMEDRIVE': 'D:', 'HOMEPATH': '\\home\\u'}, {'HOMEPATH': '\\home\\u'},
              {}, {'HOME': '/home/posix'},
              {'USERPROFILE': 'C:\\Users\\u', 'USERNAME': 'u'}]
    for p in ['~', '~/.cache', '~\\.cache', '~other\\x', 'x\\~']
]

# ------------------------------------------------- realpath fixture (POSIX)
root = os.path.realpath(FIXTURE)
os.makedirs(os.path.join(root, 'a', 'b'), exist_ok=True)
with open(os.path.join(root, 'a', 'f'), 'w') as f:
    f.write('x')
LINKS = [
    ('link_a', 'a'), ('link_abs', os.path.join(root, 'a', 'b')),
    ('link_dotdot', 'a/b/..'), ('loop1', 'loop2'), ('loop2', 'loop1'),
    ('self', 'self'), ('dangling', 'nonexistent'), ('link_file', 'a/f'),
    ('chain', 'link_a'), ('a/b/up', '../..'),
]
for name, target in LINKS:
    p = os.path.join(root, name)
    if os.path.lexists(p):
        os.unlink(p)
    os.symlink(target, p)


def rp(rel):
    p = rel.replace('$ROOT', root)
    saved = os.getcwd()
    os.chdir(root)
    try:
        return posixpath.realpath(p).replace(root, '$ROOT')
    finally:
        os.chdir(saved)


V['posixpath_realpath_fixture'] = {
    'dirs': ['a', 'a/b'],
    'files': ['a/f'],
    'symlinks': [[n, t.replace(root, '$ROOT')] for n, t in LINKS],
    'cwd': '$ROOT',
    'cases': [
        {'in': p, 'out': rp(p)}
        for p in ['$ROOT', '$ROOT/a', '$ROOT/link_a', '$ROOT/link_a/b',
                  '$ROOT/link_a/..', '$ROOT/link_abs', '$ROOT/link_abs/..',
                  '$ROOT/link_dotdot', '$ROOT/loop1', '$ROOT/loop1/x',
                  '$ROOT/self', '$ROOT/dangling', '$ROOT/dangling/x',
                  '$ROOT/link_file', '$ROOT/link_file/x', '$ROOT/chain/b',
                  '$ROOT/missing/../a', '$ROOT/missing/x', '$ROOT/a/b/up',
                  '$ROOT/a/b/up/a', '$ROOT/./a//b/', 'link_a', '.', '',
                  'missing', '$ROOT/a/f/..', '$ROOT/missing/../link_a',
                  '$ROOT/missing/../dangling', '$ROOT/link_a/../link_file']
    ],
}


# ------------------------------------------------- more error-string formats
import subprocess  # noqa: E402

V['stdlib_calledprocesserror_str'] = [
    {'returncode': rc, 'cmd': tag(cmd), 'out': str(subprocess.CalledProcessError(rc, cmd))}
    for rc, cmd in [
        (128, ('git', 'rev-list', 'refs/heads/a', '--topo-order', '--reverse',
               '--not', '--remotes=origin')),
        (1, ('git', 'rev-parse', 'x^')), (-9, ('git', 'x')), (-15, ('git', 'x')),
        (-99, ('git',)), (1, 'git x'), (2, ['git', "it's"]),
    ]
]
V['keyerror_str'] = [{'in': k, 'out': str(KeyError(k))} for k in ['x', "it's", '']]
V['argparse_invalid_choice'] = [
    {'value': v, 'choices': list(c),
     'out': 'invalid choice: %r (choose from %s)' % (v, ', '.join(map(repr, c)))}
    for v, c in [('x', ('pre-commit', 'pre-push')),
                 ("it's", ('auto', 'always', 'never'))]
]


# ------------------------------------------------- lang_base.exe_exists (pure part)
SHIMS_RE = re.compile(r'[/\\]shims[/\\]')


def exe_exists_pure(mod, found, homedir):
    try:
        common = mod.commonpath((found, homedir))
    except ValueError:
        common = None
    return (not SHIMS_RE.search(found) and
            (mod.dirname(homedir) == homedir or common != homedir))


V['exe_exists_pure'] = [
    {'flavor': fl, 'found': f, 'home': h,
     'out': exe_exists_pure(posixpath if fl == 'posix' else ntpath, f, h)}
    for fl, f, h in [
        ('posix', '/usr/bin/node', '/home/u'),
        ('posix', '/home/u/.nvm/versions/node/v20/bin/node', '/home/u'),
        ('posix', '/home/u/.pyenv/shims/python', '/home/u'),
        ('posix', '/opt/x/shims/ruby', '/home/u'),
        ('posix', '/usr/bin/node', '/'),
        ('posix', '/home/u/bin/node', '/'),
        ('posix', 'bin/node', '/home/u'),
        ('posix', '/home/u/bin/node', '/home/u/'),
        ('posix', '/home/u2/node', '/home/u'),
        ('posix', '/home/u/node', ''),
        ('nt', 'C:\\Program Files\\nodejs\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'C:\\Users\\u\\AppData\\Roaming\\npm\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'c:\\users\\U\\scoop\\shims\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'C:\\users\\u\\x\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'D:\\tools\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'C:\\Users\\u/bin\\node.EXE', 'C:\\Users\\u'),
        ('nt', 'C:\\x\\node.EXE', 'C:\\'),
    ]
]


# ------------------------------------------------- main._adjust_args_and_chdir
def adjust(mod, cwd, toplevel, arg, exists, kind):
    os.getcwd = lambda: cwd  # type: ignore[assignment]
    try:
        try:
            a = mod.abspath(arg) if (kind == 'files' or exists) else arg
        except ValueError as e:
            return err(e)
        os.getcwd = lambda: toplevel  # type: ignore[assignment]
        try:
            r = mod.relpath(a)
        except ValueError as e:
            return err(e)
        out = {'ok': r}
        if mod is ntpath and kind == 'files':
            out['classifier'] = r.replace('\\', '/')
        return out
    finally:
        os.getcwd = _real_getcwd  # type: ignore[assignment]


V['adjust_args_and_chdir'] = [
    {'flavor': fl, 'cwd': c, 'toplevel': t, 'kind': k, 'arg': a, 'exists': e,
     'out': adjust(posixpath if fl == 'posix' else ntpath, c, t, a, e, k)}
    for fl, c, t, k, a, e in [
        ('posix', '/r/sub', '/r', 'config', '.pre-commit-config.yaml', False),
        ('posix', '/r/sub', '/r', 'config', 'my.yaml', True),
        ('posix', '/r/sub', '/r', 'config', '../.pre-commit-config.yaml', True),
        ('posix', '/r/sub', '/r', 'config', '/etc/x.yaml', True),
        ('posix', '/r/sub', '/r', 'config', '', False),
        ('posix', '/r/sub', '/r', 'files', 'a.py', True),
        ('posix', '/r/sub', '/r', 'files', '../b.py', True),
        ('posix', '/r/sub', '/r', 'files', 'missing.py', False),
        ('posix', '/r/sub', '/r', 'files', '/outside/c.py', True),
        ('posix', '/r/sub', '/r', 'files', 'd/../e.py', True),
        ('posix', '/r/sub', '/r', 'files', '', False),
        ('posix', '/r', '/r', 'files', './x//y.py', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'config', '.pre-commit-config.yaml', False),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'config', 'my.yaml', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'files', 'a/b.py', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'files', '..\\c.py', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'files', 'D:\\x.py', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'files', 'c:\\R\\SUB\\x.py', True),
        ('nt', 'C:\\r\\sub', 'C:\\r', 'config', 'D:\\cfg.yaml', True),
    ]
]


# ------------------------------------------------- parse_shebang.find_executable (fixture)
def setup_fe(root):
    for d in ('bin1', 'bin2', 'bin3/tool', 'rel', 'extd'):
        os.makedirs(os.path.join(root, d), exist_ok=True)
    for f, mode in (('bin1/tool', 0o755), ('bin2/tool', 0o644), ('rel/tool2', 0o755),
                    ('tool3', 0o755), ('extd/tool.sh', 0o755), ('extd/tool', 0o755),
                    ('bin1/both', 0o755)):
        p = os.path.join(root, f)
        with open(p, 'w') as fh:
            fh.write('#!/bin/sh\n')
        os.chmod(p, mode)


def find_executable(exe, env):  # parse_shebang.py:22-43 (verbatim logic)
    exe = os.path.normpath(exe)
    if os.sep in exe:
        return exe
    environ = env
    if 'PATHEXT' in environ:
        exts = environ['PATHEXT'].split(os.pathsep)
        possible_exe_names = tuple(f'{exe}{ext}' for ext in exts) + (exe,)
    else:
        possible_exe_names = (exe,)
    for path in environ.get('PATH', '').split(os.pathsep):
        for possible_exe_name in possible_exe_names:
            joined = os.path.join(path, possible_exe_name)
            if os.path.isfile(joined) and os.access(joined, os.X_OK):
                return joined
    else:
        return None


fe_root = os.path.realpath(FIXTURE) + '/fe'
os.makedirs(fe_root, exist_ok=True)
setup_fe(fe_root)


def fe(exe, env):
    env = {k: v.replace('$ROOT', fe_root) for k, v in env.items()}
    saved = os.getcwd()
    os.chdir(fe_root)
    try:
        r = find_executable(exe, env)
        return None if r is None else r.replace(fe_root, '$ROOT')
    finally:
        os.chdir(saved)


V['find_executable_posix_fixture'] = {
    'dirs': ['bin1', 'bin2', 'bin3/tool', 'rel', 'extd'],
    'files_mode': [['bin1/tool', '755'], ['bin2/tool', '644'], ['rel/tool2', '755'],
                   ['tool3', '755'], ['extd/tool.sh', '755'], ['extd/tool', '755'],
                   ['bin1/both', '755']],
    'cwd': '$ROOT',
    'cases': [
        {'exe': x, 'env': e, 'out': fe(x, e)}
        for x, e in [
            ('tool', {'PATH': '$ROOT/bin2:$ROOT/bin1'}),
            ('tool', {'PATH': '$ROOT/bin3:$ROOT/bin1'}),
            ('tool', {'PATH': '$ROOT/bin2'}),
            ('tool2', {'PATH': 'rel'}),
            ('tool2', {'PATH': './rel/'}),
            ('tool3', {'PATH': ''}),
            ('tool3', {}),
            ('tool3', {'PATH': ':'}),
            ('tool3', {'PATH': '$ROOT/bin1:'}),
            ('tool3', {'PATH': '$ROOT/bin1'}),
            ('tool', {'PATH': '$ROOT/extd', 'PATHEXT': '.sh:.py'}),
            ('tool', {'PATH': '$ROOT/extd', 'PATHEXT': ''}),
            ('tool', {'PATH': '$ROOT/bin1:$ROOT/extd', 'PATHEXT': '.sh'}),
            ('a/b', {'PATH': '$ROOT/bin1'}),
            ('./tool3', {'PATH': '$ROOT/bin1'}),
            ('bin1/../tool3', {'PATH': ''}),
            ('tool/..', {'PATH': '$ROOT/bin1'}),
            ('', {'PATH': '$ROOT/bin1'}),
            ('nonexistent', {'PATH': '$ROOT/bin1'}),
        ]
    ],
}


# ------------------------------------------------- seeded random corpora
import random  # noqa: E402

rng = random.Random(20261008)
TXT_ALPHA = ['a', 'b', 'Z', '0', ' ', ' ', '\t', '\n', '\r', '\x0b', '\x0c', '\x1c',
             '\x1d', '\x1e', '\x1f', '\x85', '\xa0', '\u1680', '\u2000', '\u200a',
             '\u200b', '\u2028', '\u2029', '\u202f', '\u205f', '\u3000', '\ufeff',
             '\u180e', '#', ',', ':', "'", '"', '\\', '$', '\x00', '\x7f', '\xe9',
             '\U0001f600', '.', '/']


def rtext(n):
    return ''.join(rng.choice(TXT_ALPHA) for _ in range(rng.randint(0, n)))


rand_text = [rtext(12) for _ in range(400)]
V['random_str_ops'] = [
    {'in': t,
     'splitlines': t.splitlines(), 'splitlines_keepends': t.splitlines(True),
     'split': t.split(), 'split_max1': t.split(None, 1),
     'rsplit_max3': t.rsplit(None, 3), 'strip': t.strip(), 'lstrip': t.lstrip(),
     'rstrip': t.rstrip(), 'repr': repr(t), 'shlex_quote': shlex.quote(t),
     'shlex_split': call(shlex.split, t), 'textwrap_indent': textwrap.indent(t, '  '),
     'skips': get_skips(t)}
    for t in rand_text
]

BYTE_ALPHA = [0x00, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x1C, 0x20, 0x23, 0x41, 0x7F,
              0x80, 0x85, 0x9F, 0xA0, 0xBF, 0xC0, 0xC1, 0xC2, 0xC3, 0xDF, 0xE0,
              0xE2, 0xED, 0xEF, 0xF0, 0xF4, 0xF5, 0xFF]


def rbytes(n):
    return bytes(rng.choice(BYTE_ALPHA) for _ in range(rng.randint(0, n)))


rand_bytes = [rbytes(10) for _ in range(400)]
V['random_bytes_ops'] = [
    {'in_hex': b.hex(),
     'splitlines_hex': [x.hex() for x in b.splitlines()],
     'splitlines_keepends_hex': [x.hex() for x in b.splitlines(True)],
     'strip_hex': b.strip().hex(), 'rstrip_crlf_hex': b.rstrip(b'\r\n').hex(),
     'decode': call(lambda b=b: b.decode()),
     'decode_backslashreplace': b.decode(errors='backslashreplace'),
     'merge_conflicts': merge_conflicts(b)}
    for b in rand_bytes
]

P_ALPHA = ['a', 'b', '.', '..', '', '~', 'x.y', '.z']


def rposix():
    parts = [rng.choice(P_ALPHA) for _ in range(rng.randint(0, 5))]
    lead = rng.choice(['', '', '/', '//', '///'])
    return lead + '/'.join(parts)


def rnt():
    parts = [rng.choice(P_ALPHA + ['A', 'c:']) for _ in range(rng.randint(0, 5))]
    lead = rng.choice(['', '', 'C:', 'c:\\', 'C:/', '\\', '/', '\\\\srv\\sh\\',
                       '//srv/sh/', '\\\\?\\C:\\', 'D:'])
    sep = rng.choice(['\\', '/'])
    return lead + sep.join(parts)


rand_posix = [(rposix(), rposix()) for _ in range(400)]
V['random_posixpath'] = [
    {'a': a, 'b': b, 'normpath': posixpath.normpath(a), 'join': posixpath.join(a, b),
     'split': list(posixpath.split(a)), 'splitext': list(posixpath.splitext(a)),
     'abspath': with_cwd(POSIX_CWD, posixpath.abspath, a),
     'relpath': with_cwd(POSIX_CWD, posixpath.relpath, a, b) if b else
     with_cwd(POSIX_CWD, posixpath.relpath, a),
     'commonpath': call(posixpath.commonpath, [a, b])}
    for a, b in rand_posix
]
rand_nt = [(rnt(), rnt()) for _ in range(400)]
V['random_ntpath'] = [
    {'a': a, 'b': b, 'normpath': ntpath.normpath(a), 'join': ntpath.join(a, b),
     'split': list(ntpath.split(a)), 'splitext': list(ntpath.splitext(a)),
     'splitdrive': list(ntpath.splitdrive(a)), 'isabs': ntpath.isabs(a),
     'normcase': ntpath.normcase(a),
     'abspath': with_cwd(NT_CWD, ntpath.abspath, a),
     'relpath': with_cwd(NT_CWD, ntpath.relpath, a, b) if b else
     with_cwd(NT_CWD, ntpath.relpath, a),
     'real_windows_may_differ': nt_real_may_differ(a, b or None),
     'commonpath': call(ntpath.commonpath, [a, b])}
    for a, b in rand_nt
]
RL_PIECES = ['', ' ', '  ', '\t', '\xa0', '\x1f', '　', '\x85', 'rev:', 'rev', 'Rev:',
             "'", '"', '#', 'v1.0', 'abc', '[x]', '# frozen: v1', '\r', '\n', '\r\n',
             ' ', 'x y', '\x0c']


def rline():
    return ''.join(rng.choice(RL_PIECES) for _ in range(rng.randint(1, 8)))


rand_rl = []
while len(rand_rl) < 600:
    ln = rline()
    if len(rand_rl) % 2 == 0:  # bias half of the corpus towards near-matches
        ln = rng.choice(['  ', '\t', ' \xa0']) + 'rev:' + rline() + rng.choice(['\n', '\r\n', ''])
    rand_rl.append(ln)
V['random_rev_lines'] = [{'in': ln, 'groups': rev_match(ln)} for ln in rand_rl]

V['decimal_zeros'] = [cp for cp in range(0x110000)
                      if unicodedata.decimal(chr(cp), None) == 0]

# ================================================= WP-11 extension sections
# Everything below `decimal_zeros` was added by WP-11 for the production API of
# ARCHITECTURE §1.4.2 (random, eaw, float, the bytes composites, case folding).
# It uses its own RNG, so the original sections above are unaffected.
import hashlib  # noqa: E402
import math  # noqa: E402
import signal  # noqa: E402
import struct  # noqa: E402

EXT_SECTIONS = [
    'random_mt19937', 'random_randbelow', 'random_shuffle', 'random_shuffle_strings',
    'float_round2', 'float_repr', 'east_asian_width_wf', 'len_cjk', 'int_parse',
    'signal_names', 'repr_codepoints', 'ntpath_more', 'ntpath_pairs', 'text_io_random',
    'str_split_random', 'bytes_split_random', 'pre_push_parse_bytes',
]


def _fnv1a64(data: bytes) -> str:
    h = 0xcbf29ce484222325
    for byte in data:
        h ^= byte
        h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return '%016x' % h


def _bits(x: float) -> str:
    return struct.pack('>d', x).hex()


FIXED_RANDOM_SEED = 1542676187  # lang_base.py:22


def _shuffled(seq):  # lang_base.py:146-153
    fixed_random = random.Random()
    fixed_random.seed(FIXED_RANDOM_SEED, version=1)
    seq = list(seq)
    fixed_random.shuffle(seq)
    return seq


def _key_words(seed: int) -> list[int]:
    # random_seed() in _randommodule.c: the 32-bit words of abs(seed), least significant first
    n, words = abs(seed), []
    while n:
        words.append(n & 0xFFFFFFFF)
        n >>= 32
    return words or [0]


def _len_cjk(msg: str) -> int:  # commands/run.py:36-38
    widths = {'A': 1, 'F': 2, 'H': 1, 'N': 1, 'Na': 1, 'W': 2}
    return sum(widths[unicodedata.east_asian_width(c)] for c in msg)


def _duration(x: float) -> str:  # commands/run.py:202,221
    return f'{round(x, 2) or 0}'


def _signal_name(n: int):
    try:
        return signal.Signals(n).name
    except ValueError:
        return None


def _extension() -> None:
    ext = random.Random(20261009)

    # ---- random.Random (MT19937): seeding, raw output, _randbelow, shuffle
    V['random_mt19937'] = [
        {'seed': str(s), 'key': _key_words(s),
         'genrand': [r.getrandbits(32) for _ in range(8)]}
        for s in [0, 1, 5489, FIXED_RANDOM_SEED, 2**32 - 1, 2**32, 2**64 + 5, -7,
                  12345678901234567890123]
        for r in [random.Random(s)]
    ]
    r = random.Random(FIXED_RANDOM_SEED)
    V['random_randbelow'] = {
        'seed': FIXED_RANDOM_SEED,
        'cases': [[n, [r._randbelow(n) for _ in range(6)]]
                  for n in [1, 2, 3, 5, 7, 10, 100, 1000, 2**16 + 1, 2**31 - 1, 2**31,
                            2**32 - 1, 2**32, 2**32 + 1, 2**40 + 3, 2**53 + 1, 2**63,
                            2**64 - 1]],
    }
    ns = sorted(set(range(65)) | {100, 1000, 2000, 5000} | set(range(0, 2001, 37)))
    shuffles = []
    for n in ns:
        out = _shuffled(range(n))
        entry = {'n': n, 'fnv1a64': _fnv1a64(','.join(map(str, out)).encode())}
        if n <= 100:
            entry['out'] = out
        shuffles.append(entry)
    V['random_shuffle'] = shuffles
    seq = [str(i) for i in range(10)]
    V['random_shuffle_strings'] = {'in': seq, 'out': _shuffled(seq)}  # T10-XRG-16

    # ---- float: f'{round(x, 2) or 0}' and repr(float); inputs are IEEE-754 bit patterns
    def finite_bits() -> float:
        while True:
            x = struct.unpack('>d', ext.getrandbits(64).to_bytes(8, 'big'))[0]
            if math.isfinite(x):
                return x

    special = [
        0.0, -0.0, math.inf, -math.inf, math.nan, 5e-324, -5e-324, 2.2250738585072014e-308,
        1.7976931348623157e308, -1.7976931348623157e308, 1.0, 0.1, 0.001, -0.001, 0.004,
        0.0049999999999999, 0.005, 0.0050000000000001, 0.0051, 0.125, 0.375, 0.625, 0.875,
        2.675, 1.005, 1.015, 1.025, 0.285, 1.0049999999999999, 12.5, 0.77, 1.236, 99.995,
        999.995, 1e15, 1e16, 1e17, 1e22, 1e23, 123456789.125, 4503599627370495.5,
        9007199254740993.0, 0.0001, 1e-05, 1.5e-07, -0.125, -2.675, -0.005, 0.015, 0.025,
    ]
    ties = ([(2 * k + 1) / 200 for k in range(2000)] + [m / 8 for m in range(400)]
            + [-(2 * k + 1) / 200 for k in range(0, 200, 7)])
    rand = ([ext.random() for _ in range(2500)] + [ext.uniform(0, 100) for _ in range(2500)]
            + [ext.expovariate(0.5) for _ in range(2500)] + [finite_bits() for _ in range(2500)])
    V['float_round2'] = [f'{_bits(x)} {_duration(x)}' for x in special + ties + rand]
    tens = [float(f'1e{e}') for e in range(-325, 309)]
    edges = [9.999999999999999e15, 9999999999999998.0, 1e16 - 2, 0.0001, 9.999999999999999e-05,
             0.00011, 123456789012345680.0, 1.25e22, 2.0**53, 2.0**63, 2.0**-1074, 0.3,
             1 / 3, 2 / 3, 100.0, 1e-07]
    # doubles between 2**49 and 2**64 have short exact expansions, so exact ties between the two
    # closest 17-digit candidates are common there (dtoa breaks them to the even digit)
    dense = [struct.unpack('>d', (((0x430 + ext.randrange(16)) << 52) | ext.getrandbits(52))
                           .to_bytes(8, 'big'))[0] for _ in range(2000)]
    halves = [(2 * ext.getrandbits(53) + 1) / 2 ** ext.randrange(1, 12) for _ in range(500)]
    V['float_repr'] = [f'{_bits(x)} {x!r}' for x in
                       special + edges + tens + [finite_bits() for _ in range(3000)]
                       + dense + halves]

    # ---- east-asian width (commands/run.py:36-38)
    V['east_asian_width_wf'] = ranges(
        lambda cp: unicodedata.east_asian_width(chr(cp)) in ('W', 'F'))
    V['len_cjk'] = [
        {'in': s, 'out': _len_cjk(s)}
        for s in ['', 'passing hook', '检查中文名称の長いフック', 'emoji 🎉 é combining é',
                  '\u200b', 'e\u0301', '\uff21\uff22', '\uffe0x', '\u3000', '\u2e80',
                  '\U0001f600\U0001f1fa\U0001f1f8', '\u1100\u115f\u1160', '\x00\x7f\x85',
                  'ｱｲｳ', '\u20a9', '\U00020000\U0003fffd\U0002ebf0']
    ]

    # ---- int() (base 10) with the %.200R error text
    int_in = [
        '0', '-0', '+0', '00', '007', '1_000', '1__0', '_1', '1_', '+_1', '-_1', '1_0_0',
        ' 42 ', '\t-17\n', '\x1c5\x1f', '\xa012\u3000', '\u200b1', '1\u200b', '- 1', '+-1',
        '--1', '++1', '0x10', '0b1', '0o7', '1.0', '1e3', '', ' ', '-', '+', '_',
        '\u0661\u0662\u0663', '\u0661_\u0662', '\U0001d7cf\U0001d7d0', '\u0663.1',
        '12\u0660', '\xb2', '\xbd', '\u216b', '\u1369', '\u0660x1', 'abc', "it's",
        'say "hi"', 'both \'"', '\x00', '1\x00', '\xe9', '9' * 38, '-' + '9' * 38,
        '170141183460469231731687303715884105727', '-170141183460469231731687303715884105728',
        'x' * 300, "'" * 250, '"' * 250, '\x00' * 150, '\xe9' * 300,
        '\U0001fae9' * 30 + 'x' * 100, 'a\tb' * 80, '\u0966\u0967', '\uff11\uff10',
        '\u2007', '1\u20072', '٣', '1 000',
    ]
    int_cases = []
    for s in int_in:
        out = call(int, s)
        if 'ok' in out and not -2**127 <= out['ok'] < 2**127:
            continue  # beyond i128: parse_int reports these (documented)
        int_cases.append({'in': tag(s), 'out': out})
    V['int_parse'] = int_cases

    # ---- signal.Signals names of the generating platform (subprocess's "died with <Signals.X: n>")
    V['signal_names'] = {'platform': sys.platform,
                         'names': [[n, _signal_name(n)] for n in range(0, 70)]}

    # ---- repr() of single code points around every printability boundary class
    cps = (list(range(0x0, 0x300)) + list(range(0x2000, 0x2070)) + list(range(0x2ff0, 0x3001))
           + list(range(0x31e0, 0x31f0)) + list(range(0xd7f0, 0xd800)) + list(range(0xdc80, 0xdd00))
           + list(range(0xe000, 0xe010)) + list(range(0xfdd0, 0xfdf0)) + [0xfeff]
           + list(range(0xfff0, 0x10000)) + list(range(0x1f600, 0x1f610))
           + list(range(0x1fae0, 0x1fb00)) + list(range(0x2ebf0, 0x2ebf6))
           + list(range(0x2ee5c, 0x2ee61)) + list(range(0xe0000, 0xe0081, 16)) + [0xf0000]
           + [0x10fffd, 0x10fffe, 0x10ffff])
    V['repr_codepoints'] = [{'in': tag(chr(cp)), 'out': repr(chr(cp))} for cp in cps]

    # ---- ntpath: more roots (UNC, devices, non-ASCII drives) and Unicode case folding
    nt_more = [
        '\\\\?\\UNC\\srv\\share\\x', '\\\\?\\unc\\srv\\share', '//?/UNC/srv/share/x/../y',
        '\\\\?\\UNC\\srv', '\\\\?\\UNC\\srv\\share', '\\\\.\\COM1', '\\\\.\\pipe\\name\\..\\x',
        '\u00e9:x', '\u00e9:\\x', '\U0001f600:x', '\U0001f600:\\x\\..\\y', 'C:', 'c:..\\..',
        '\\\\srv\\share\\..', '\\\\srv\\\\share', '\\\\\\x', '\\\\srv\\share\\', '///x', 'C:/',
        'C:\\\\x\\\\y', '\u00c9LODIE\\\u03a3\u0391\u03a3', 'C:\\Users\\\u00c9lodie',
        'c:\\users\\\u00e9lodie\\x', '\u0130\\x', '\u01c5:\\x', '\u212a\\x', '\u1e9e:\\x',
        '\\\\S\u00c9RVER\\Share\\x', 'x\\\u039f\u0394\u039f\u03a3\\y', '\u03a3\u0391\u03a3',
        'a.b\\c.', 'C:\\a\\b.tar.gz', 'C:a.b', '\\\\srv\\share.x\\y.z', 'C:\\a\\..\\..\\..',
        '\\\\srv\\share\\a\\..\\..\\..', 'C:\\x\\.\\.\\y\\', '..\\\\..', 'a\\..\\..\\b',
    ]
    V['ntpath_more'] = [
        {'in': p, 'splitroot': list(ntpath.splitroot(p)), 'normpath': ntpath.normpath(p),
         'split': list(ntpath.split(p)), 'splitext': list(ntpath.splitext(p)),
         'splitdrive': list(ntpath.splitdrive(p)), 'isabs': ntpath.isabs(p),
         'normcase': ntpath.normcase(p), 'expanduser_profile': expanduser_env(
             {'USERPROFILE': 'C:\\Users\\\u00c9lodie', 'USERNAME': '\u00c9lodie'},
             '~' + p, ntpath)['ok']}
        for p in nt_more
    ]
    nt_pairs = [
        ('C:\\Users\\\u00c9lodie\\repo\\x', 'c:\\users\\\u00e9lodie\\repo'),
        ('\\\\S\u00c9RVER\\Share\\x', '\\\\s\u00e9rver\\share'),
        ('C:\\\u039f\u0394\u039f\u03a3\\x', 'c:\\\u03bf\u03b4\u03bf\u03c2'),
        ('C:\\\u039f\u0394\u039f\u03a3\\x', 'c:\\\u03bf\u03b4\u03bf\u03c3'),
        ('C:\\\u039f\u0394\u039f\u03a3x\\y', 'c:\\\u03bf\u03b4\u03bf\u03c3x'),
        ('\u0130:\\x', 'i\u0307:\\y'), ('\u00e9:x', '\u00c9:y'), ('\u00e9:\\a\\b', '\u00c9:\\A'),
        ('\\\\?\\UNC\\srv\\share\\x', '\\\\?\\unc\\SRV\\share\\y'), ('C:\\a', 'D:\\a'),
        ('\u01c5:\\x\\y', '\u01c6:\\x'), ('C:\\K\\x', 'c:\\\u212a'), ('C:\\\u212a\\x', 'c:\\k'),
        ('\\\\srv\\share', '\\\\srv\\share\\x'), ('C:x', 'c:y'), ('C:\\a\\b', 'C:'),
        ('\\a\\b', '\\a'), ('a\\b', 'a'), ('C:\\Stra\u00dfe\\x', 'c:\\STRASSE'),
    ]
    V['ntpath_pairs'] = [
        {'a': a, 'b': b, 'cwd': NT_CWD, 'join': ntpath.join(a, b), 'join_rev': ntpath.join(b, a),
         'relpath': with_cwd(NT_CWD, ntpath.relpath, a, b),
         'commonpath': call(ntpath.commonpath, [a, b])}
        for a, b in nt_pairs
    ]

    # ---- text-mode newline translation on random text
    tio_alpha = ['a', '\u00e9', '\r', '\n', '\r\n', '\x85', '\u2028', '\x0b', '\x0c', ' ', '\r\r']
    tio = []
    for _ in range(400):
        s = ''.join(ext.choice(tio_alpha) for _ in range(ext.randint(0, 12)))
        b = s.encode()
        tio.append({'in': s, 'newline_none': read_universal(b),
                    'windows_hex': write_translate(s, '\r\n').hex()})
    V['text_io_random'] = tio

    # ---- str.split / rsplit / partition / strip(chars) on random text
    rows = []
    for _ in range(400):
        t = ''.join(ext.choice(TXT_ALPHA) for _ in range(ext.randint(0, 14)))
        rows.append({
            'in': t, 'split0': t.split(None, 0), 'split2': t.split(None, 2),
            'split3': t.split(None, 3), 'rsplit0': t.rsplit(None, 0),
            'rsplit1': t.rsplit(None, 1), 'rsplit': t.rsplit(), 'partition': list(t.partition(',')),
            'strip_chars': t.strip('#,\\ '), 'lstrip_chars': t.lstrip('#,\\ '),
            'rstrip_chars': t.rstrip('#,\\ '),
        })
    V['str_split_random'] = rows

    # ---- bytes.rsplit(None, m) / bytes.split() on random bytes
    b_alpha = [b'a', b'b', b' ', b'\t', b'\n', b'\r', b'\x0b', b'\x0c', b'\x1c', b'\x1f',
               b'\x85', b'\xa0', b'\xc2\xa0', b'\xe9', b'\x00', b'#']
    brows = []
    for _ in range(400):
        b = b''.join(ext.choice(b_alpha) for _ in range(ext.randint(0, 12)))
        brows.append({'in_hex': b.hex(),
                      **{f'rsplit_{m}': [x.hex() for x in b.rsplit(None, m)] for m in (-1, 0, 1, 3)}})
    V['bytes_split_random'] = brows

    # ---- the bytes variant of the pre-push parse (D-CRASH-11): bytes.splitlines + rsplit(maxsplit=3)
    def pre_push_parse_bytes(stdin: bytes) -> list:
        out = []
        for line in stdin.splitlines():
            parts = line.rsplit(maxsplit=3)
            try:
                a, b, c, d = parts
            except ValueError as e:
                out.append({'line_hex': line.hex(), **err(e)})
                break
            out.append({'line_hex': line.hex(), 'fields_hex': [x.hex() for x in (a, b, c, d)]})
        return out

    pp_alpha = [b'refs/heads/x', b'0' * 40, b'1' * 40, b' ', b'  ', b'\t', b'\x0b', b'\x0c',
                b'\r', b'\n', b'\r\n', b'\x1c', b'\x85', b'\xa0', b'\xe9', b'\x00', b'(delete)']
    pp_in = list(PP) + [
        b''.join(ext.choice(pp_alpha) for _ in range(ext.randint(0, 10))) for _ in range(300)
    ]
    V['pre_push_parse_bytes'] = [{'in_hex': b.hex(), 'out': pre_push_parse_bytes(b)} for b in pp_in]

    assert list(V)[-len(EXT_SECTIONS):] == EXT_SECTIONS, list(V)


if not LEGACY:
    _extension()


def _delta(base: dict, cur: dict) -> dict:
    out: dict = {}
    for name in base:
        if name != 'meta' and name not in cur:
            sys.exit(f'section {name} missing from the current interpreter')
    for name, val in cur.items():
        if name == 'meta':
            continue
        b = base.get(name)
        if b is None:
            sys.exit(f'section {name} missing from the base file')
        if b == val:
            continue
        if isinstance(val, list) and isinstance(b, list) and len(b) == len(val):
            out[name] = {str(i): v for i, (x, v) in enumerate(zip(b, val)) if x != v}
        elif isinstance(val, dict) and isinstance(b, dict) and list(b) == list(val):
            out[name] = {k: v for k, v in val.items() if b[k] != v}
        else:
            # e.g. a range list of another length: the whole section is replaced
            out[name] = {'*': val}
    return out


if DELTA_BASE is not None:
    with open(DELTA_BASE, 'rb') as fh:
        base_bytes = fh.read()
    base = json.loads(base_bytes)
    current = json.loads(json.dumps(V, ensure_ascii=True))  # tuples -> lists, as in the file
    doc = {
        'meta': {
            'generator': 'xtask/oracle/gen_pycompat_vectors.py',
            'python': platform.python_version(),
            'implementation': platform.python_implementation(),
            'unicode': unicodedata.unidata_version,
            'platform': sys.platform,
            'base': os.path.basename(DELTA_BASE),
            'base_python': base['meta']['python'],
            'base_unicode': base['meta']['unicode'],
            'base_sha256': hashlib.sha256(base_bytes).hexdigest(),
            'notes': (
                'delta[section] maps a list index (decimal string) or an object key of the '
                'base section to the value this interpreter produces, or "*" to the whole '
                'section when its shape changed; sections and entries that are equal are '
                'omitted'
            ),
        },
        'delta': _delta(base, current),
    }
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(doc, f, ensure_ascii=True, indent=1)
        f.write('\n')
    print('wrote', OUT, 'delta sections:', len(doc['delta']))
else:
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(V, f, ensure_ascii=True, indent=1)
        f.write('\n')
    print('wrote', OUT, 'sections:', len(V))
