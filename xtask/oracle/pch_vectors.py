"""snp-pyparsers vectors (WP-17): CPython's json / difflib / configparser / tomllib / expat and ruamel.yaml >= 0.19
behaviour, recorded for the Rust ports in crates/snp-pyparsers.

Run by `cargo xtask gen-pch-data vectors` (never by hand in CI) under the pch oracles: CPython 3.12.3 and 3.13.x
venvs with exactly `pre-commit-hooks==6.0.0` and `ruamel.yaml==0.19.1` (`cargo xtask gen-pch-data oracle` builds
them). The corpora (JSONTestSuite, yaml-test-suite, toml-test, the W3C XML conformance suite) are fetched and
verified by `pch_corpora.py`. Every vector file is deterministic: the same interpreter and corpora give the same
bytes.

usage: python -I pch_vectors.py --minor 312|313 --corpora DIR --out DIR [--only json,difflib,...]
"""
from __future__ import annotations

import argparse
import base64
import configparser
import difflib
import io
import json
import os
import random
import subprocess
import sys
import tempfile
import xml.sax
from typing import Any

GENERATOR = 'xtask/oracle/pch_vectors.py'


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode('ascii')


def blob(b: bytes) -> dict[str, Any]:
    """Raw input bytes: `{"b64": …}`, or for large periodic inputs `{"repeat": [b64(unit), count], "tail": b64}`
    (`tests/conformance/support.rs` `input()` rebuilds them)."""
    if len(b) > 16384:
        for p in range(1, 17):
            unit = b[:p]
            count = len(b) // p
            while count and b[:p * count] != unit * count:
                count -= 1
            if count * p >= len(b) - 64:
                return {'repeat': [b64(unit), count], 'tail': b64(b[p * count:])}
    return {'b64': b64(b)}


def fnv1a64(s: str) -> str:
    """FNV-1a (64 bit) of the UTF-8 bytes, as 16 hex digits: large expected outputs are stored as a digest."""
    h = 0xcbf29ce484222325
    for byte in s.encode('utf-8'):
        h = ((h ^ byte) * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return f'{h:016x}'


def text_out(key: str, s: str) -> dict[str, Any]:
    """An expected text output: verbatim, or `{key + "_fnv": digest, key + "_len": len}` beyond 4 KiB."""
    if len(s) > 4096:
        return {f'{key}_fnv': fnv1a64(s), f'{key}_len': len(s)}
    return {key: s}


def has_surrogate(s: str) -> bool:
    return any(0xD800 <= ord(c) <= 0xDFFF for c in s)


def crash(exc: BaseException) -> dict[str, Any]:
    text = str(exc)
    return {'crash': [type(exc).__name__, text.encode('utf-8', 'backslashreplace').decode('utf-8')]}


def reported(msg: str) -> dict[str, Any]:
    """A message the hook prints: printing a lone surrogate raises UnicodeEncodeError (a crash)."""
    try:
        msg.encode('utf-8')
    except UnicodeEncodeError as e:
        return crash(e)
    return {'reported': msg}


def script(name: str) -> str:
    return os.path.join(os.path.dirname(sys.executable), name)


def write_json(out: str, name: str, data: dict[str, Any]) -> None:
    path = os.path.join(out, name)
    with open(path, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(data, f, ensure_ascii=True, indent=1, sort_keys=False)
        f.write('\n')
    print(f'wrote {path}', file=sys.stderr)


def header(kind: str) -> dict[str, Any]:
    return {
        'generator': GENERATOR,
        'kind': kind,
        'python': '.'.join(map(str, sys.version_info[:3])),
    }


# --------------------------------------------------------------------------------------------------------------
# json
# --------------------------------------------------------------------------------------------------------------

def raise_duplicate_keys(ordered_pairs):  # check_json.py (v4.0.0+), verbatim semantics
    d = {}
    for key, val in ordered_pairs:
        if key in d:
            raise ValueError(f'Duplicate key: {key}')
        else:
            d[key] = val
    return d


def check_json_result(data: bytes) -> dict[str, Any]:
    try:
        json.load(io.BytesIO(data), object_pairs_hook=raise_duplicate_keys)
    except ValueError as exc:
        return reported(str(exc))
    except Exception as exc:  # noqa: BLE001 - every other exception is the crash path
        return crash(exc)
    return {'ok': True}


def lossy(s: str) -> str:
    return ''.join('\ufffd' if 0xD800 <= ord(c) <= 0xDFFF else c for c in s)


def tree(v: Any) -> list[Any]:
    """A decoded value as a flat token list in prefix order (deep values need no recursion in either language):
    `"n"`, `"t"`, `"f"`, `["i", digits]`, `["d", repr(float)]`, `["s", str]`, `["a", len]` + items,
    `["o", len]` + (`["k", key]`, value) pairs. Lone surrogates become U+FFFD, as in the Rust `JsonValue`."""
    out: list[Any] = []
    todo = [v]
    while todo:
        x = todo.pop()
        if isinstance(x, KeyToken):
            out.append(['k', lossy(x.key)])
        elif x is None:
            out.append('n')
        elif x is True:
            out.append('t')
        elif x is False:
            out.append('f')
        elif isinstance(x, int):
            out.append(['i', str(x)])
        elif isinstance(x, float):
            out.append(['d', repr(x)])
        elif isinstance(x, str):
            out.append(['s', lossy(x)])
        elif isinstance(x, Pairs):
            out.append(['o', len(x)])
            for k, val in reversed(x):
                todo.append(val)
                todo.append(KeyToken(k))
        elif isinstance(x, list):
            out.append(['a', len(x)])
            todo.extend(reversed(x))
        else:
            raise TypeError(type(x))
    return out


class KeyToken:
    def __init__(self, key: str) -> None:
        self.key = key


class Pairs(list):
    pass


def loads_result(data: bytes) -> dict[str, Any]:
    try:
        v = json.loads(data, object_pairs_hook=Pairs)
    except ValueError as exc:
        return reported(str(exc))
    except Exception as exc:  # noqa: BLE001
        return crash(exc)
    return {'value': tree(v)}


def json_handcrafted() -> list[tuple[str, bytes]]:
    """09 §4.C catalogue plus edge cases of encodings, escapes, numbers and delimiters."""
    cases: list[tuple[str, bytes]] = []
    add = lambda name, b: cases.append((name, b if isinstance(b, bytes) else b.encode('utf-8', 'surrogatepass')))
    for i, s in enumerate([
        '', '  ', ' \n ', '{"a": 1} {}', '01', '1.', '1e', '.5', '-', 'tru', '-NaN', '{"a": 1, "a": 2',
        '{\n  "a": 1\n  "b": 2\n}', '[1 2]', '{"a"  1}', '{"a"', "{'a': 1}", '{', '{"a": 1,}', '[1,]',
        '{"a":}', '"abc', '{"a": "x', '{"a": "x\ty"}', '{"a":\n "\u00e9\u00e9\n', '"\\x"', '"\\u12"',
        '"\\uZZZZ"', '{"a": {"b": 1, "b": 2}, "a": 3}', '{"\u00e9":1,"\u00e9":2}', 'NaN', 'Infinity',
        '-Infinity', '[NaN, Infinity, -Infinity]', '1e999', '-1e999', '[1e-400]', '"\\ud800"',
        '"\\ud800\\udc00"', '"\\udc00\\ud800"', '"\\ud800\\u0041"', '"\\ud800\\uZZZZ"', '"\\ud800\\"',
        '"\\ud800\\u12"', '"\\u1234', '"\\u123', '"\\ud834\\udd1e"', '["\\ud834\\udd1e"]', '"\\/\\b\\f\\n\\r\\t"',
        '"\\', '"\\"', '"a\\', '"\x7f"', '"\x1f"', '"\x00"', '"\u2028"', '[]', '{}', '[ ]', '{ }', '[\n]',
        ' [ 1 , 2 ] ', '[1,2', '[1,', '[', ']', '}', '{"a":1}}', '[1]]', '{"a" : 1 , "b" : [ ] }', 'null',
        'true', 'false', 'nul', 'nulls', 'truex', 'falsey', 'True', 'None', '-0', '-0.0', '0.0', '1E5',
        '1e+5', '1e-5', '1E+', '1E-', '1e+x', '[1e]', '[1e+]', '[-]', '[-1]', '[--1]', '[+1]', '[0x10]',
        '[00]', '[0.1.2]', '[1.e5]', '[1.5e]', '[.5]', '["a" "b"]', '{"a":1 "b":2}', '{"a" 1}', '{1: 2}',
        '{"a":1,,"b":2}', '[1,,2]', '[,1]', '{,"a":1}', '{"a":1, }', '[1, ]', '{"a":1,\n}', '[1,\n]',
        '{"a":1} x', '[] []', '\ufeff[]', '[1]\x00', '\x00', '[\x00]', '["\x00"]', '\u00a0[]', '[]\u00a0',
        '\t\r\n []\t\r\n ', '\x0c[]', '[\x0b]', '"\u00e9', '{"\u00e9":"\u00e9", "\u00e9": 1}', '["a\\u0000b"]',
        '{"":1, "":2}', '{"a":[{"b":1,"b":2}]}', '{"a":1,"b":{"a":1},"a":3}', '{"x": {"y": 1, "y": 2}, "x" 3}',
        '[' + '1' * 4300 + ']', '[' + '1' * 4301 + ']', '-' + '1' * 4301, '[' + '1' * 5000 + ', x]',
        '1' * 4301 + '.5', '1' * 4301 + 'e5', '0' + '1' * 4301,
    ]):
        add(f'hand-{i:03d}', s)
    # Encodings (json.detect_encoding) and decode errors (surrogatepass).
    for name, b in [
        ('utf8-bom', b'\xef\xbb\xbf[1]'), ('utf8-bom-only', b'\xef\xbb\xbf'), ('utf8-bom-bad', b'\xef\xbb\xbf["\xff"]'),
        ('utf16le-bom', '\ufeff[1, "\u00e9"]'.encode('utf-16-le')), ('utf16be-bom', '\ufeff[1]'.encode('utf-16-be')),
        ('utf16le', '[1, "\u00e9"]'.encode('utf-16-le')), ('utf16be', '[1, "x"]'.encode('utf-16-be')),
        ('utf16le-odd', '[1]'.encode('utf-16-le') + b'\x00'), ('utf16be-odd', '[1]'.encode('utf-16-be') + b'['),
        ('utf32le-bom', '\ufeff[1]'.encode('utf-32-le')), ('utf32be-bom', '\ufeff[1]'.encode('utf-32-be')),
        ('utf32le', '[1]'.encode('utf-32-le')), ('utf32be', '[1]'.encode('utf-32-be')),
        ('utf32le-bad', b'[\x00\x00\x00\x00\x00\x11\x00]\x00\x00\x00'),
        ('utf32le-trunc', '[1]'.encode('utf-32-le') + b'\x00'),
        ('utf16le-lone-surrogate', b'"\x00\x00\xd8"\x00'), ('utf16be-lone-surrogate', b'\x00"\xdc\x00\x00"'),
        ('utf16le-2bytes', b'1\x00'), ('utf16be-2bytes', b'\x001'), ('one-byte', b'1'), ('three-bytes', b'[1]'),
        ('utf16le-3', b'1\x00 '), ('nul-first-4', b'\x00\x00\x00['), ('nul-second', b'[\x00\x00\x00'),
        ('bad-start', b'["\xff"]'), ('bad-cont', b'["\xc3("]'), ('trunc', b'["\xe2\x82'), ('trunc-end', b'\xe2\x82'),
        ('surrogate-utf8', b'["\xed\xa0\x80"]'), ('surrogate-pair-utf8', b'["\xed\xa0\xbd\xed\xb8\x80"]'),
        ('surrogate-utf8-trunc', b'["\xed\xa0'), ('surrogate-utf8-key-dup', b'{"\xed\xa0\x80":1,"\xed\xa0\x80":2}'),
        ('overlong', b'["\xc0\xaf"]'), ('too-big', b'["\xf4\x90\x80\x80"]'), ('f5', b'["\xf5"]'),
        ('e0-bad', b'["\xe0\x80\x80"]'), ('f0-bad', b'["\xf0\x80\x80\x80"]'), ('lone-cont', b'["\x80"]'),
        ('bad-after-ok', b'["\xc3\xa9\xc3\xa9", "\xff\xfe"]'), ('cp1252', b'["caf\xe9"]'),
        ('dup-surrogate-escape', b'{"\\ud800":1,"\\ud800":2}'), ('dup-surrogate-mixed', b'{"\\ud800":1,"\xed\xa0\x80":2}'),
        ('pair-vs-cesu', b'{"\\ud83d\\ude00":1,"\xed\xa0\xbd\xed\xb8\x80":2}'),
        ('pair-vs-utf8', b'{"\\ud83d\\ude00":1,"\xf0\x9f\x98\x80":2}'),
    ]:
        add(f'enc-{name}', b)
    return cases


def json_random_docs(rng: random.Random, n: int) -> list[str]:
    """Random valid JSON documents (strings with every escape class, numbers of every shape, duplicates)."""
    keys = ['a', 'b', 'c', 'B', 'aa', '', '\u00e9', '\U0001f600', 'z', '1', 'a b', 'A']
    chars = 'abc xyz\u00e9\u00fc\u4e2d\U0001f600"\\/\b\f\n\r\t\x00\x1f\x7f\u2028\u2029\ufeff'

    def string() -> str:
        return ''.join(rng.choice(chars) for _ in range(rng.randrange(0, 8)))

    def number() -> Any:
        k = rng.randrange(9)
        if k == 0:
            return rng.randrange(-10**6, 10**6)
        if k == 1:
            return rng.choice([0, -0.0, 0.0, 1e16, 1e-5, 1e-4, 123456789012345678, 1.5, -2.5e-7, 1e22, 1e300])
        if k == 2:
            return rng.uniform(-1e6, 1e6)
        if k == 3:
            return float(rng.randrange(-10**9, 10**9)) / (10 ** rng.randrange(0, 12))
        if k == 4:
            return rng.choice([float('inf'), float('-inf'), float('nan')])
        if k == 5:
            return 10 ** rng.randrange(15, 40)
        if k == 6:
            return rng.random() * 10 ** rng.randrange(-30, 30)
        if k == 7:
            return rng.choice([True, False, None])
        return int.from_bytes(rng.randbytes(rng.randrange(1, 20)), 'big')

    def value(depth: int) -> Any:
        k = rng.randrange(10 if depth < 5 else 6)
        if k < 3:
            return string()
        if k < 6:
            return number()
        if k < 8:
            return [value(depth + 1) for _ in range(rng.randrange(0, 5))]
        pairs = [(rng.choice(keys), value(depth + 1)) for _ in range(rng.randrange(0, 6))]
        return pairs

    def render(v: Any, pretty: bool) -> str:
        if isinstance(v, list) and v and all(isinstance(p, tuple) for p in v):
            inner = ', '.join(json.dumps(k) + ': ' + render(x, pretty) for k, x in v)
            return '{' + inner + '}'
        if isinstance(v, list):
            if v == []:
                return rng.choice(['[]', '{}'])
            return '[' + ','.join(render(x, pretty) for x in v) + ']'
        return json.dumps(v, ensure_ascii=rng.random() < 0.5)

    out = []
    for i in range(n):
        v = value(0)
        doc = render(v, False)
        if i % 3 == 0:
            # Re-render through Python's own pretty printer so some inputs are already pretty.
            try:
                doc = json.dumps(json.loads(doc), indent=rng.choice([2, 4]), ensure_ascii=False) + '\n'
            except (ValueError, TypeError):
                pass
        if i % 7 == 0:
            doc = doc.replace('\n', '\r\n')
        out.append(doc)
    return out


def measure(script_name: str, make: Any, classify: Any, lo: int, hi: int) -> int:
    """The largest n in [lo, hi) for which `classify(output)` is False (no RecursionError)."""
    def crashes(n: int) -> bool:
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, 'f.json')
            with open(p, 'w', encoding='utf-8') as f:
                f.write(make(n))
            r = subprocess.run([script(script_name), p], capture_output=True, text=True)
        return classify(r.stdout + r.stderr)

    assert not crashes(lo), (script_name, lo)
    assert crashes(hi), (script_name, hi)
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if crashes(mid):
            hi = mid
        else:
            lo = mid
    return lo


def recursion(out: str) -> bool:
    return 'RecursionError' in out


# Deep-nesting document shapes (mirrored by crates/snp-pyparsers/tests/json.rs `shape()`): each is a document whose
# only interesting event happens at nesting depth n.
SHAPES = {
    'arrays': lambda n: '[' * n + ']' * n,                                      # enter n containers, valid
    'stop': lambda n: '[' * n + 'x',                                            # StopIteration inside n containers
    'eof': lambda n: '[' * n,                                                   # StopIteration at EOF inside n
    'errmsg': lambda n: '[' * n + '1 2',                                        # JSONDecodeError inside n
    'errmsg_str': lambda n: '[' * n + '"\x01"',                                 # JSONDecodeError from scanstring
    'int': lambda n: '[' * n + '1' * 4301,                                      # int-limit ValueError inside n
    'const': lambda n: '[' * n + 'NaN' + ']' * n,                               # parse_constant call inside n
    'hook': lambda n: '[' * (n - 1) + '{"a":1}' + ']' * max(n - 11, 0) + ' x',  # pairs hook at n, error at 10
    'hook_raise': lambda n: '[' * (n - 1) + '{"a":1,"a":2}' + ']' * (n - 1),   # check-json's hook raising at n
    'objects': lambda n: '{"a":' * n + '1' + '}' * n,                           # objects, valid
    'objects_err': lambda n: '{"a":' * n + '1' + '}' + 'x' + '}' * (n - 1),      # hook at n, error at n - 1
    'mixed': lambda n: '[{"a":' * (n // 2) + '[]' + '}]' * (n // 2),            # alternating, valid
}


def json_limits() -> dict[str, int]:
    """Nesting thresholds of the real console scripts (09 §4.C, Q9): the largest n at which each shape still
    behaves as it would without a recursion limit."""
    lim = {}
    for prefix, name, shapes in [
        ('check', 'check-json', ['arrays', 'stop', 'errmsg', 'errmsg_str', 'int', 'const', 'hook', 'hook_raise']),
        ('pretty', 'pretty-format-json', ['stop', 'errmsg', 'errmsg_str', 'int', 'const', 'hook', 'arrays',
                                          'objects']),
    ]:
        for shape in shapes:
            lim[f'{prefix}_{shape}'] = measure(name, SHAPES[shape], recursion, 20, 30000)
    return lim


def json_hook_outcome(script_name: str, doc: str) -> dict[str, Any]:
    """The outcome of the real console script on one file: reported message, crash line, or ok."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, 'f.json')
        with open(p, 'w', encoding='utf-8') as f:
            f.write(doc)
        r = subprocess.run([script(script_name), p], capture_output=True, text=True)
    out = (r.stdout + r.stderr).strip()
    if 'Traceback' in out or 'Normalization failed' in out:
        line = [ln for ln in out.splitlines() if ln.startswith('RecursionError')]
        return {'crash': ['RecursionError', line[-1].split(': ', 1)[1] if line else '']}
    if r.returncode == 0:
        return {'ok': True}
    if script_name == 'check-json':
        prefix = f'{p}: Failed to json decode ('
        assert out.startswith(prefix) and out.endswith(')'), out
        return {'reported': out[len(prefix):-1]}
    if out.startswith('Input File'):
        return {'reported': 'invalid'}
    return {'ok': True}  # pretty-format-json printed a diff: the document parsed and encoded


def json_depth_cases(lim: dict[str, int]) -> list[dict[str, Any]]:
    """Every shape around every threshold with the real scripts' outcomes; runs of equal outcomes are thinned to
    the cases next to a change."""
    cases = []
    ns = set()
    for b in set(lim.values()):
        ns.update({b - 1, b, b + 1, b + 2})
    for mode, name in [('check', 'check-json'), ('pretty', 'pretty-format-json')]:
        for shape in SHAPES:
            if mode == 'pretty' and shape == 'hook_raise':
                continue
            for n in sorted(ns):
                cases.append({
                    'mode': mode, 'shape': shape, 'n': n, 'result': json_hook_outcome(name, SHAPES[shape](n)),
                })
    out = []
    for i, c in enumerate(cases):
        prev = cases[i - 1] if i else None
        nxt = cases[i + 1] if i + 1 < len(cases) else None

        def same(o: Any) -> bool:
            return o is not None and o['mode'] == c['mode'] and o['shape'] == c['shape'] and o['result'] == c['result']

        if not (same(prev) and same(nxt)):
            out.append(c)
    return out


def get_pretty(contents: str, opts: dict[str, Any]) -> dict[str, Any]:
    from pre_commit_hooks.pretty_format_json import _get_pretty_format

    try:
        pretty = _get_pretty_format(
            contents, opts['indent'], ensure_ascii=opts['ensure_ascii'], sort_keys=opts['sort_keys'],
            top_keys=opts['top_keys'],
        )
    except ValueError as exc:
        return {'reported': str(exc).encode('utf-8', 'backslashreplace').decode('utf-8')}
    except Exception as exc:  # noqa: BLE001
        return crash(exc)
    try:
        pretty.encode('utf-8')
    except UnicodeEncodeError as exc:
        return crash(exc)
    return text_out('pretty', pretty)


PRETTY_OPTS = [
    {'indent': 2, 'ensure_ascii': True, 'sort_keys': True, 'top_keys': []},
    {'indent': 4, 'ensure_ascii': False, 'sort_keys': True, 'top_keys': []},
    {'indent': '\t', 'ensure_ascii': True, 'sort_keys': False, 'top_keys': []},
    {'indent': 0, 'ensure_ascii': False, 'sort_keys': False, 'top_keys': []},
    {'indent': -2, 'ensure_ascii': True, 'sort_keys': True, 'top_keys': ['b', 'a']},
    {'indent': '', 'ensure_ascii': True, 'sort_keys': True, 'top_keys': ['z']},
    {'indent': 'ab', 'ensure_ascii': False, 'sort_keys': True, 'top_keys': ['a', 'a', '']},
    {'indent': 2, 'ensure_ascii': True, 'sort_keys': False, 'top_keys': ['c', 'b']},
]


def json_vectors(corpora: str) -> dict[str, Any]:
    suite = os.path.join(corpora, 'JSONTestSuite', 'test_parsing')
    raw: list[tuple[str, bytes]] = []
    for fn in sorted(os.listdir(suite)):
        if fn.endswith('.json'):
            with open(os.path.join(suite, fn), 'rb') as f:
                raw.append((f'suite-{fn}', f.read()))
    raw += json_handcrafted()
    inputs = [
        {'name': n, **blob(b), 'check': check_json_result(b), 'loads': loads_result(b)} for n, b in raw
    ]

    pretty = []
    rng = random.Random(20261008)
    for i, doc in enumerate(json_random_docs(rng, 500)):
        opts = PRETTY_OPTS[i % len(PRETTY_OPTS)]
        pretty.append({'name': f'random-{i:03d}', 'text': doc, 'opts': opts, 'result': get_pretty(doc, opts)})
    for idx, (name, b) in enumerate(raw):
        try:
            text = b.decode('utf-8')
        except UnicodeDecodeError:
            continue
        text = text.replace('\r\n', '\n').replace('\r', '\n')  # open(..., encoding='UTF-8').read()
        if has_surrogate(text) or len(text) > 20000:
            continue
        for j, opts in enumerate(PRETTY_OPTS[:3] + PRETTY_OPTS[6:]):
            pretty.append({'name': f'{name}-opt{j}', 'input': idx, 'opts': opts, 'result': get_pretty(text, opts)})
    for name, text, opts in PRETTY_HAND:
        pretty.append({'name': f'hand-{name}', 'text': text, 'opts': opts, 'result': get_pretty(text, opts)})

    lim = json_limits()
    data = header('json')
    data['limits'] = lim
    data['inputs'] = inputs
    data['pretty'] = pretty
    data['depth'] = json_depth_cases(lim)
    return data


PRETTY_HAND = [
    ('dup-sorted', '{"a": 1, "a": 2}', PRETTY_OPTS[0]), ('dup-sorted-rev', '{"x": 1, "x": 0}', PRETTY_OPTS[0]),
    ('dup-unsorted', '{"a": 1, "b": 0, "a": 2}', PRETTY_OPTS[2]),
    ('dup-incomparable', '{"a": 1, "a": {}}', PRETTY_OPTS[0]),
    ('dup-incomparable-unsorted', '{"a": 1, "a": {}}', PRETTY_OPTS[2]),
    ('dup-none', '{"a": null, "a": null}', PRETTY_OPTS[0]), ('dup-list', '{"a": [1, 2], "a": [1]}', PRETTY_OPTS[0]),
    ('dup-list-mixed', '{"a": [1, "x"], "a": [1, 2]}', PRETTY_OPTS[0]),
    ('dup-list-eq', '{"a": [1, {}], "a": [1, {}]}', PRETTY_OPTS[0]),
    ('dup-list-prefix', '{"a": [1, 2, 3], "a": [1, 2]}', PRETTY_OPTS[0]),
    ('dup-bool-int', '{"a": true, "a": 1, "a": 0.5}', PRETTY_OPTS[0]),
    ('dup-nan', '{"a": NaN, "a": 1}', PRETTY_OPTS[0]), ('dup-nan-nan', '{"a": NaN, "a": NaN}', PRETTY_OPTS[0]),
    ('dup-str-int', '{"a": "1", "a": 1}', PRETTY_OPTS[0]),
    ('dup-big', '{"a": 100000000000000000000000000000, "a": 1e30}', PRETTY_OPTS[0]),
    ('dup-big-eq', '{"a": 9007199254740993, "a": 9007199254740992.0}', PRETTY_OPTS[0]),
    ('dup-inf', '{"a": 1e999, "a": 100000000000000000000000000000000000000000}', PRETTY_OPTS[0]),
    ('dup-str', '{"a": "b", "a": "a", "a": "c"}', PRETTY_OPTS[0]),
    ('dup-str-unicode', '{"a": "\\u00e9", "a": "z", "a": "\\ud83d\\ude00", "a": "\\uffff"}', PRETTY_OPTS[0]),
    ('dup-three', '{"k": 3, "j": 0, "k": 1, "k": 2}', PRETTY_OPTS[0]),
    ('dup-nested-crash-first', '{"a": {"x": 1, "x": {}}, ', PRETTY_OPTS[0]),
    ('dup-inner-then-error', '[{"x": 1, "x": []}, x]', PRETTY_OPTS[0]),
    ('top-keys', '{"c": 1, "b": 2, "a": 3, "z": 4}', PRETTY_OPTS[4]),
    ('top-keys-dup', '{"b": 1, "a": 2, "b": 3}', PRETTY_OPTS[4]),
    ('top-keys-nested', '{"x": {"b": 1, "q": 2, "a": 3}, "b": [{"a": 1, "b": 2}]}', PRETTY_OPTS[4]),
    ('bom', '\ufeff{"a": 1}', PRETTY_OPTS[0]), ('neg-zero-int', '[-0, -0.0, 0]', PRETTY_OPTS[0]),
    ('floats', '[1E5, 1e2, 0.10, -0.0, 1e16, 1.5e-7, 1e22, 1E400, -1e400, 1e-400, 123456789.123456789, 5e-324,'
     ' 1.7976931348623157e308, 0.0001, 0.00001, 9999999999999999.0, 1e15, 123e-20]', PRETTY_OPTS[0]),
    ('big-int', '[' + '9' * 4300 + ']', PRETTY_OPTS[0]), ('too-big-int', '[' + '9' * 4301 + ']', PRETTY_OPTS[0]),
    ('ascii', '{"a": "\\u0000\\u001f\u007f \\ud800\u00e9\U0001f600\\"\\\\/\\b\\f\\n\\r\\t"}', PRETTY_OPTS[0]),
    ('no-ascii', '{"a": "\\u0000\\u001f\u007f \u00e9\U0001f600\u2028\\"\\\\/\\b\\f\\n\\r\\t"}', PRETTY_OPTS[1]),
    ('no-ascii-surrogate', '["\\ud800"]', PRETTY_OPTS[1]),
    ('no-ascii-surrogate-key', '{"\\udc00": 1}', PRETTY_OPTS[1]),
    ('ascii-surrogate-key', '{"\\udc00": 1, "\\ud800\\udc00": 2}', PRETTY_OPTS[0]),
    ('empty-containers', '{"a": [], "b": {}, "c": [[]], "d": [{}]}', PRETTY_OPTS[0]),
    ('already-pretty', '{\n  "a": 1\n}\n', PRETTY_OPTS[0]),
    ('scalar', '"x"', PRETTY_OPTS[0]), ('nan-top', 'NaN', PRETTY_OPTS[0]),
]


# --------------------------------------------------------------------------------------------------------------
# difflib
# --------------------------------------------------------------------------------------------------------------

def difflib_vectors() -> dict[str, Any]:
    """`''.join(difflib.unified_diff(a, b, fromfile=f, tofile=f))` on line lists as `str.splitlines(True)` makes
    them: pretty-format-json shaped pairs, random edits over small alphabets (many equal lines, hunk merging,
    the autojunk heuristic from 200 lines on) and exotic line ends."""
    rng = random.Random(1542676187)
    cases = []

    def add(name: str, a: list[str], b: list[str], f: str = 'f.json') -> None:
        out = ''.join(difflib.unified_diff(a, b, fromfile=f, tofile=f))
        cases.append({'name': name, 'a': a, 'b': b, 'file': f, **text_out('diff', out)})

    for i, doc in enumerate(json_random_docs(random.Random(77), 150)):
        try:
            pretty = json.dumps(json.loads(doc), indent=2) + '\n'
        except (ValueError, TypeError):
            continue
        add(f'pretty-{i:03d}', doc.splitlines(True), pretty.splitlines(True))
    alphabets = [['a\n', 'b\n', 'c\n'], ['x\n', 'y\n', 'z\n', 'w\n', 'v\n', 'u\n', 't\n'], ['{\n', '}\n', '  "a": 1,\n', '  "b": [\n', '  ],\n']]
    for i in range(400):
        alpha = alphabets[i % len(alphabets)]
        n = rng.choice([0, 1, 2, 3, 5, 8, 13, 21, 40, 70]) if i % 10 else rng.choice([120, 199, 200, 201, 260, 450])
        a = [rng.choice(alpha) for _ in range(n)]
        b = list(a)
        for _ in range(rng.randrange(0, 12)):
            op = rng.randrange(3)
            pos = rng.randrange(len(b) + 1)
            if op == 0:
                b.insert(pos, rng.choice(alpha + [f'new{rng.randrange(5)}\n']))
            elif op == 1 and b:
                del b[min(pos, len(b) - 1)]
            elif b:
                b[min(pos, len(b) - 1)] = rng.choice(alpha + ['changed\n'])
        if i % 5 == 0 and b:
            b[-1] = b[-1].rstrip('\n')
        if i % 7 == 0 and a:
            a[-1] = a[-1].rstrip('\n')
        add(f'random-{i:03d}', a, b, rng.choice(['f.json', 'dir/x y.json', 'é.json', '']))
    for name, a, b in [
        ('empty-both', [], []), ('empty-a', [], ['x\n']), ('empty-b', ['x\n'], []), ('same', ['a\n', 'b\n'], ['a\n', 'b\n']),
        ('no-newline', ['{"a":1}'], ['{\n', '  "a": 1\n', '}\n']),
        ('exotic-ends', 'a\x0bb\x0cc\x1cd e\x85f\n'.splitlines(True), 'a\nb\x0cc d\n'.splitlines(True)),
        ('far-apart', [f'{k}\n' for k in range(60)], [f'{k}\n' if k not in (5, 50) else 'X\n' for k in range(60)]),
        ('near', [f'{k}\n' for k in range(20)], [f'{k}\n' if k not in (5, 11) else 'X\n' for k in range(20)]),
        ('popular', ['p\n'] * 250 + ['q\n'], ['p\n'] * 249 + ['r\n', 'q\n']),
    ]:
        add(f'hand-{name}', a, b)
    data = header('difflib')
    data['cases'] = cases
    return data


# --------------------------------------------------------------------------------------------------------------
# tomllib (check-toml)
# --------------------------------------------------------------------------------------------------------------

def frame_depth() -> int:
    """The number of Python frames on the stack of the caller of this function."""
    depth = 0
    f = sys._getframe(1)
    while f is not None:
        depth += 1
        f = f.f_back
    return depth


def toml_result(data: bytes) -> dict[str, Any]:
    """`check_toml.main` on one file, with the recursion headroom of the real console script: there
    `tomllib.loads` runs at frame depth 4 (`<module>`, `main`, `tomllib.load`, `loads`) under a limit of 1000."""
    import tomllib

    try:
        text = data.decode()
    except UnicodeDecodeError as exc:
        return crash(exc)
    depth = frame_depth() + 1  # tomllib.loads' own frame
    old = sys.getrecursionlimit()
    sys.setrecursionlimit(1000 - 4 + depth)
    try:
        tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return reported(str(exc))
    except Exception as exc:  # noqa: BLE001
        return crash(exc)
    finally:
        sys.setrecursionlimit(old)
    return {'ok': True}


def toml_hook_outcome(doc: str) -> dict[str, Any]:
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, 'f.toml')
        with open(p, 'w', encoding='utf-8', newline='') as f:
            f.write(doc)
        r = subprocess.run([script('check-toml'), p], capture_output=True, text=True)
    out = (r.stdout + r.stderr).strip()
    if 'Traceback' in out:
        last = out.splitlines()[-1]
        ty, _, msg = last.partition(': ')
        return {'crash': [ty, msg]}
    if r.returncode == 0:
        return {'ok': True}
    assert out.startswith(f'{p}: '), out
    return {'reported': out[len(p) + 2:]}


TOML_SHAPES = {
    'arr-int': lambda n: 'a = ' + '[' * n + '1' + ']' * n,
    'arr-empty': lambda n: 'a = ' + '[' * n + ']' * n,
    'arr-true': lambda n: 'a = ' + '[' * n + 'true' + ']' * n,
    'arr-str': lambda n: 'a = ' + '[' * n + '"x"' + ']' * n,
    'arr-mlstr': lambda n: 'a = ' + '[' * n + '"""x\\u0041"""' + ']' * n,
    'arr-date': lambda n: 'a = ' + '[' * n + '1979-05-27T07:32:00+01:00' + ']' * n,
    'arr-err': lambda n: 'a = ' + '[' * n + 'x',
    'arr-comment': lambda n: 'a = ' + '[#c\n' * n + ']' * n,
    'tbl-int': lambda n: 'a = ' + '{b = ' * n + '1' + '}' * n,
    'tbl-empty': lambda n: 'a = ' + '{b = ' * n + '{}' + '}' * n,
    'tbl-quoted': lambda n: 'a = ' + '{"b" = ' * n + '1' + '}' * n,
    'tbl-dup': lambda n: 'a = ' + '{b = ' * n + '{c = 1, c = 2}' + '}' * n,
    'mixed': lambda n: 'a = ' + '[{b = ' * n + '1' + '}]' * n,
}


def toml_vectors(corpora: str) -> dict[str, Any]:
    suite = os.path.join(corpora, 'toml-test', 'tests')
    raw: list[tuple[str, bytes]] = []
    for sub in ('valid', 'invalid'):
        for dirpath, _, files in sorted(os.walk(os.path.join(suite, sub))):
            for fn in sorted(files):
                if fn.endswith('.toml'):
                    path = os.path.join(dirpath, fn)
                    with open(path, 'rb') as f:
                        raw.append((os.path.relpath(path, suite), f.read()))
    hand = [
        'a = 1\na = 2', 'a.b = 1\na = 2', 'a = \n', 'x = [1,\n2,\n', '﻿a = 1', 'a = { b = 1, }', 'a = {\n b = 1 }',
        'a = 1 # \x07', '[a]\n[a]', 'a = 1979-05-27T07:32', 'a = 12:30', 'a = 1979-13-27', 'a = 0x', 'a = "\\e"',
        'a = 1\rb = 2', '', 'a = 1\r\nb = 2\r\n', 'a = 99999999999999999999', 'a = ' + '9' * 4300, 'a = ' + '9' * 4301,
        'a = -' + '9' * 4301, 'a = ' + '1_' * 4300 + '1', 'a = 0x' + 'f' * 5000, 'a = ' + '9' * 4301 + '.0',
        'a = 2001-02-29', 'a = 2000-02-29', 'a = 0000-01-01', 'a = 2001-04-31T00:00:00Z', 'a = 1979-05-27 07:32:00',
        'a = 1979-05-27 07:32', 'a = 1979-05-27T07:32:00.123456789+23:59', 'a = 1979-05-27T07:32:00+24:00',
        'a = 1979-05-27T07:32:00-07:00', 'a = 07:32:00.5', 'a = 24:00:00', 'a = inf', 'a = -nan', 'a = +inf', 'a = infinity',
        'a = 1e', 'a = 1e+', 'a = 1.e5', 'a = 1_', 'a = 1__0', 'a = 01', 'a = +0x1', 'a = 0b102', 'a = 0o78', 'a = 0xG',
        'a = "\\u0041\\U0001F600"', 'a = "\\uD800"', 'a = "\\U00110000"', 'a = "\\u12"', 'a = "\\', 'a = "abc',
        'a = """\nabc\\\n   def"""', 'a = """abc\\  \n  x"""', 'a = """abc\\  x"""', 'a = """a""""', 'a = """a"""""',
        'a = """a""""""', "a = '''a''''", "a = '''\nx\x01'''", "a = 'x\x7f'", 'a = "x\x7f"', 'a = "\t"', 'a = "\x00"',
        '# \r\n', '# x\ry', 'a = [1, 2,]', 'a = [1 2]', 'a = [,]', 'a = [', 'a = {', 'a = {b = 1, b = 2}',
        'a = {b.c = 1, b = 2}', 'a = {b = [1], b.c = 2}', 'a = {b = {}, b.c = 1}', '[a]\nb = 1\n[a.b]', '[a.b]\n[a]',
        '[a]\n[a.b]\n[a]', '[[a]]\n[a]', '[a]\n[[a]]', 'a = []\n[[a]]', 'a = [{}]\n[[a]]', '[[a.b]]\n[a]\nb.c = 1',
        'a.b.c = 1\na.b.d = 2\n[a]', 'a.b = 1\n[a.b]', '[a.b.c]\n[a]\nb.c.d = 1', 'a = {b = 1}\na.c = 2', '[ a . b ]',
        '[a]b = 1', '[[a]] x', '[a', '[[a]', '[]', '[[]]', 'a b = 1', 'a. = 1', '.a = 1', 'a = 1 b = 2', '"a" = 1\n\'a\' = 2',
        'a = true1', 'a = truefalse', 'a = fals', '= 1', 'a.' + '.'.join(['x'] * 999) + ' = 1',
        'a.' + '.'.join(['x'] * 1000) + ' = 1', '[' + '.'.join(['x'] * 1001) + ']', 'a = """\n\x0b"""',
        'a\t=\t1\t#\tc', 'a = 1\n\n\n[b]\n\n c = 2', 'x = 1\n[[x]]',
    ]
    for i, h in enumerate(hand):
        raw.append((f'hand-{i:03d}', h.encode('utf-8', 'surrogatepass')))
    raw.append(('hand-bad-utf8', b'a = "\xff"'))
    raw.append(('hand-bad-utf8-comment', b'# \xc3\x28\na = 1'))
    rng = random.Random(1000)
    valid = [b for n, b in raw if n.startswith('valid')]
    alphabet = b'[]{}=.,"\'#\n \t\\x1-:_0aTZ+e'
    for i in range(400):
        b = bytearray(rng.choice(valid))
        for _ in range(rng.randrange(1, 4)):
            op = rng.randrange(3)
            p = rng.randrange(len(b) + 1)
            if op == 0:
                b[p:p] = bytes([rng.choice(alphabet)])
            elif op == 1 and b:
                del b[min(p, len(b) - 1)]
            elif b:
                b[min(p, len(b) - 1)] = rng.choice(alphabet)
        raw.append((f'mutant-{i:03d}', bytes(b)))
    cases = [{'name': n, **blob(b), 'result': toml_result(b)} for n, b in raw]
    depth = []
    for shape, make in TOML_SHAPES.items():
        # The threshold from the real script, then the cases around it.
        lo, hi = 2, 3000
        while hi - lo > 1:
            mid = (lo + hi) // 2
            if 'crash' in toml_hook_outcome(make(mid)):
                hi = mid
            else:
                lo = mid
        for n in (lo - 1, lo, lo + 1, lo + 2):
            depth.append({'shape': shape, 'n': n, 'result': toml_hook_outcome(make(n))})
    data = header('toml')
    data['cases'] = cases
    data['depth'] = depth
    return data


# --------------------------------------------------------------------------------------------------------------
# configparser (detect-aws-credentials)
# --------------------------------------------------------------------------------------------------------------

def aws_secrets(path: str) -> dict[str, Any]:
    """`detect_aws_credentials.get_aws_secrets_from_file` with the collected keys kept in order (the hook's set is
    hash-ordered), as `[(option, key), ...]`; crashes as the traceback's last line."""
    parser = configparser.ConfigParser()
    try:
        parser.read(path, encoding='utf-8')
    except configparser.MissingSectionHeaderError:
        return {'pairs': []}
    except Exception as exc:  # noqa: BLE001
        return {'crash': [f'{type(exc).__module__}.{type(exc).__qualname__}', str(exc)]}
    pairs = []
    for section in parser.sections():
        for var in ('aws_secret_access_key', 'aws_security_token', 'aws_session_token'):
            try:
                key = parser.get(section, var).strip()
            except configparser.NoOptionError:
                continue
            except Exception as exc:  # noqa: BLE001
                return {'crash': [f'{type(exc).__module__}.{type(exc).__qualname__}', str(exc)]}
            if key:
                pairs.append([var, key])
    return {'pairs': pairs}


def ini_vectors() -> dict[str, Any]:
    rng = random.Random(4242)
    hand = [
        '', '\n', '# only comment\n', 'key = value\n', '  \n[a]\n', '[a]\naws_secret_access_key = abc\n',
        '[a]\nAWS_SECRET_ACCESS_KEY: ab\n', '[a]\naws_secret_access_key=\n', '[a]\naws_secret_access_key = %%x\n',
        '[a]\naws_secret_access_key = 5%b\n', '[a]\naws_secret_access_key = %(x)s\n',
        '[a]\nx = X\naws_secret_access_key = %(x)s-%(X)s\n', '[a]\naws_secret_access_key = %(aws_secret_access_key)s\n',
        '[a]\naws_secret_access_key = %(x\n', '[a]\naws_secret_access_key = %(x)d\n', '[a]\naws_secret_access_key = %\n',
        '[a]\naws_secret_access_key = 100%\n', '[a]\n[a]\n', '[a]\nk=1\nk=2\n', '[a]\nK=1\nk=2\n', '[DEFAULT]\n[DEFAULT]\n',
        '[DEFAULT]\nk=1\n[DEFAULT]\nk=2\n', '[DEFAULT]\naws_session_token = d\n[x]\n[y]\naws_session_token = y\n',
        '[a]\nnot an option\nalso bad\n', '[a]\n= v\n', '[a]\n= v\n= w\n', '[a]\n:\n', '[a]\naws_session_token = 1\n  2\n\n  3\n#c\n  4\n',
        '[a]\naws_session_token =\n  2\n', '[a]\n  aws_session_token = indented\n', '[a]\nk = v\n    aws_session_token = c\n',
        '[a] trailing\naws_session_token = t\n', '[a]]\naws_session_token = t\n', '[]\n', '[ a ]\naws_session_token = t\n',
        '[a]\naws_session_token = t ; not a comment\n', '[a]\naws_session_token = #x\n', '[a]\n;c\naws_session_token = s\n',
        '[a]\r\naws_session_token = crlf\r\n', '[a]\raws_session_token = cr\r', '[a]\naws_session_token = \u00e9\u00e9\n',
        '[\u00e9]\nAWS_SESSION_TOKEN = x\n', '[a]\naws_session_token := v\n', '[a]\naws_session_token : = v\n',
        '[a]\naws security token = no\n', '[a]\naws_secret_access_key = a\naws_security_token = b\naws_session_token = c\n',
        '[a]\nk = v\n[b]\nk = v\n[a]\n', 'x\n[a]\n', '[a]\nk = v\n\tcontinued\n', '[a]\nk\u00a0= v\n',
        '[a]\naws_session_token = \x0bx\x0b\n', '[a]\n\u2028\naws_session_token = s\n', '[a]\naws_session_token = %(missing)s\n',
        '[a]\nb = %(c)s\nc = %(b)s\naws_session_token = %(b)s\n', '[DEFAULT]\nx = 1\n[a]\naws_session_token = %(X)s\n',
        '[a]\naws_session_token = v1\nAws_Session_Token = v2\n',
    ]
    pieces = ['[a]\n', '[b]\n', '[DEFAULT]\n', 'aws_session_token = s\n', 'aws_secret_access_key: k\n', 'x = 1\n',
              '  cont\n', '\n', '# c\n', '; c\n', 'bad line\n', '= v\n', 'y = %(x)s\n', 'z = 50%%\n', '[a]\n']
    cases = []
    with tempfile.TemporaryDirectory() as d:
        old = os.getcwd()
        os.chdir(d)
        try:
            texts = list(hand)
            for _ in range(300):
                texts.append(''.join(rng.choice(pieces) for _ in range(rng.randrange(1, 10))))
            for i, text in enumerate(texts):
                with open('creds.ini', 'w', encoding='utf-8', newline='') as f:
                    f.write(text)
                cases.append({'name': f'case-{i:03d}', 'text': text, 'result': aws_secrets('creds.ini')})
        finally:
            os.chdir(old)
    data = header('ini')
    data['cases'] = cases
    return data


# --------------------------------------------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------------------------------------------

def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument('--minor', required=True, choices=['312', '313'])
    p.add_argument('--corpora', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--only', default='json,difflib,ini,toml,xml,yaml')
    args = p.parse_args()
    want = f'3.{args.minor[1:]}'
    assert '.'.join(map(str, sys.version_info[:2])) == want, sys.version
    only = set(args.only.split(','))
    gens = {
        'json': lambda: json_vectors(args.corpora),
        'difflib': difflib_vectors,
        'ini': ini_vectors,
        'toml': lambda: toml_vectors(args.corpora),
    }
    # Modules whose code is identical in 3.12 and 3.13 (checked when the vectors were introduced) are recorded once,
    # from 3.13.
    version_independent = {'difflib', 'ini'}
    for name, gen in gens.items():
        if name not in only:
            continue
        if name in version_independent:
            if args.minor == '313':
                write_json(args.out, f'{name}.json', gen())
        else:
            write_json(args.out, f'{name}_{args.minor}.json', gen())
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
