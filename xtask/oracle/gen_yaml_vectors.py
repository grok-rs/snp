"""Generate snp-yaml's PyYAML vectors (WP-14).

Run by `cargo xtask yaml-vectors` under the pre-commit 4.6.2 oracle interpreter (target/oracle/pc462-py312:
CPython 3.12.3, PyYAML 6.0.3 with libyaml 0.2.5) as

    python -I gen_yaml_vectors.py OUT_DIR

It writes deterministic JSON files into OUT_DIR (crates/snp-yaml/tests/vectors):

* scalars.json   500+ scalars: implicit tag, load of the plain and of the explicitly tagged form, the autoupdate
                 rev re-dump in the three quote styles, and the dump of {'k': s}
* load.json      documents: the loaded value or the error (type and str), incl. 40+ error documents
* compose.json   node arenas of `yaml.compose`: tags, styles, start marks, end indexes
* dump.json      yaml_dump(yaml_load(src)) for config-like and adversarial values
* decode.json    open(f, encoding='utf-8').read() of byte strings (text or UnicodeDecodeError)
* limits.json    what PyYAML does with the documents beyond snp's limits (D-SEC-11 evidence)
* random.json    3,000 seeded random documents: the load outcome and yaml_dump(yaml_load(text))

Values are encoded canonically: ["null"], ["bool", b], ["int", "123"], ["float", repr], ["str", s],
["bytes", hex], ["date", repr], ["datetime", repr], ["list", [...]], ["tuple", [...]],
["dict", [[k, v], ...]], ["set", sorted members]. Errors are {"error": [type, str(exc)]}.
"""

import datetime
import io
import json
import os
import random
import sys
import tempfile

import yaml
from yaml.nodes import ScalarNode

assert yaml.__with_libyaml__, 'the oracle must have libyaml'
LOADER = yaml.CSafeLoader
DUMPER = yaml.CSafeDumper


def canon(o, depth=0):
    if depth > 200:
        return ['deep']
    if o is None:
        return ['null']
    if isinstance(o, bool):
        return ['bool', o]
    if isinstance(o, int):
        try:
            return ['int', str(o)]
        except ValueError:
            return ['int', hex(o)]
    if isinstance(o, float):
        return ['float', repr(o)]
    if isinstance(o, str):
        return ['str', o]
    if isinstance(o, bytes):
        return ['bytes', o.hex()]
    if isinstance(o, datetime.datetime):
        return ['datetime', repr(o)]
    if isinstance(o, datetime.date):
        return ['date', repr(o)]
    if isinstance(o, list):
        return ['list', [canon(x, depth + 1) for x in o]]
    if isinstance(o, tuple):
        return ['tuple', [canon(x, depth + 1) for x in o]]
    if isinstance(o, dict):
        return ['dict', [[canon(k, depth + 1), canon(v, depth + 1)] for k, v in o.items()]]
    if isinstance(o, set):
        members = [canon(x, depth + 1) for x in o]
        return ['set', sorted(members, key=lambda m: json.dumps(m, sort_keys=True))]
    raise TypeError(type(o))


def has_cycle(root):
    """Self-referencing structures (`&a [*a]`): snp reports them as an error (D-SEC-11). Linear in the number of
    distinct objects (aliases share objects)."""
    done = set()
    on_stack = set()
    stack = [(root, False)]
    while stack:
        o, leaving = stack.pop()
        if not isinstance(o, (list, dict, tuple, set)):
            continue
        if leaving:
            on_stack.discard(id(o))
            done.add(id(o))
            continue
        if id(o) in on_stack:
            return True
        if id(o) in done:
            continue
        on_stack.add(id(o))
        stack.append((o, True))
        items = [x for kv in o.items() for x in kv] if isinstance(o, dict) else list(o)
        for x in items:
            if isinstance(x, (list, dict, tuple, set)) and id(x) in on_stack:
                return True
            stack.append((x, False))
    return False


def expanded_size(root, cap):
    """The number of nodes of the expanded tree, stopping at cap."""
    memo = {}

    def size(o):
        if not isinstance(o, (list, dict, tuple, set)):
            return 1
        if id(o) in memo:
            return memo[id(o)]
        items = [x for kv in o.items() for x in kv] if isinstance(o, dict) else list(o)
        total = 1
        for x in items:
            total += size(x)
            if total > cap:
                break
        memo[id(o)] = min(total, cap + 1)
        return memo[id(o)]

    return size(root)


def load_result(text):
    try:
        v = yaml.load(text, Loader=LOADER)
    except RecursionError as e:
        return {'error': ['RecursionError', str(e)]}
    except Exception as e:  # noqa: BLE001 - every exception is a vector
        return {'error': [type(e).__name__, str(e)]}
    if has_cycle(v):
        return {'cycle': True}
    if expanded_size(v, 1_000_000) > 1_000_000:
        return {'too_large': True}
    return {'value': canon(v)}


def yaml_dump(o, **kw):
    return yaml.dump(o, Dumper=DUMPER, default_flow_style=False, indent=4, sort_keys=False, **kw)


def rev_dump(s, q):
    return yaml_dump({'rev': s}, default_style=q).split(':', 1)[1].strip()


def double_quoted(s):
    """A YAML double-quoted literal of s (one line)."""
    out = yaml.dump(s, Dumper=DUMPER, default_style='"', width=10**9)
    return out.rstrip('\n')


# --- scalars -------------------------------------------------------------------------------------------------------

def scalar_corpus():
    out = []
    add = out.append
    # 01 §12.2 table, 07 §3.1.3, 10 §7.5.6 and the 37 rev strings of 07 §3.1.5.
    for s in [
        'yes', 'Yes', 'YES', 'on', 'On', 'ON', 'true', 'True', 'TRUE', 'no', 'No', 'NO', 'off', 'Off', 'OFF',
        'false', 'False', 'FALSE', 'y', 'n', 'Y', 'N', 'yEs', 'tRue', 'oFF', 'nUll', '~', 'null', 'Null', 'NULL',
        '', '010', '0700', '0_1', '0800', '09', '0o10', '0o17', '017', '0x1F', '0b101', '1_000', '+12', '-0',
        '1__0', '1234567', '1:30', '190:20:30', '-1:30', '1:60', '1:20', '1.10', '1.0', '.5', '0.', '1.e+3',
        '1_0.5', '6.8523015e+5', '12:30:45.5', '190:20:30.15', '1e3', '1.0e3', '1.5e3', '1.5e+3', '1e3456',
        '123e4567', '.inf', '+.inf', '-.inf', '.Inf', '.INF', '.nan', '.NaN', '.NAN', '-.NaN', '-.5', '0.1_0',
        '1.2.3', 'v1.10', '3.2.0', '2020-01-01', '2002-01-01', '2001-12-14t21:59:43.10-05:00',
        '2001-12-14 21:59:43.10 -5', '2002-01-01T00:00:00Z', '2002-1-1', '2020-13-45', '2020-01-01 10:00:00',
        '0b_', '0x_', '=', '<<', 'v1.2.3', '2', 'abc#def', 'a #b', 'a: b', '@x', '@at', '`b', '%p', '!t', '*x',
        '&x', '!x', '%x', '&a', '*a', "'q", '"dq', 'deadbeef', '-x', 'v1.0 ', ' v1.0', 'v1.0.0',
        'x"y\'z', 'it\'s', 'a,b', '#hash', 'é', '[x]', '{x}', '0777', '0123', '12:30',
        'aa658a0cb064bf927c7cbd4c3034e4d876de3c9c', 'a' * 40, '0' * 40, '1' * 40,
    ]:
        add(s)
    # Every case variant of the bool/null words.
    for word in ['yes', 'no', 'on', 'off', 'true', 'false', 'null']:
        for mask in range(1 << len(word)):
            add(''.join(c.upper() if mask >> i & 1 else c for i, c in enumerate(word)))
    # Numbers.
    for sign in ['', '+', '-']:
        for body in ['0', '00', '1', '01', '08', '0b1', '0b2', '0x0', '0xg', '0xFF', '0XFF', '1_2', '_1', '1_',
                     '1.5', '1.', '.5', '1.5e+2', '1.5E-2', '1.5e2', '1:0', '1:00', '0:30', '1:5:9', '1:59.9',
                     '1:60.0', '.inf', '.nan', '1e+3', '9' * 30, '0x' + 'f' * 40, '0b' + '1' * 100,
                     '1' * 4301]:
            add(sign + body)
    # Timestamps.
    for t in ['2020-02-29', '2021-02-29', '0000-01-01', '9999-12-31', '2020-1-1 1:00:00', '2020-01-01T25:00:00',
              '2020-01-01 10:60:00', '2020-01-01 10:00:60', '2020-01-01 10:00:00.1234567',
              '2020-01-01 10:00:00.', '2020-01-01 10:00:00 Z', '2020-01-01 10:00:00+24:00',
              '2020-01-01 10:00:00-23:59', '2020-01-01 10:00:00 +5:30', '2020-01-01 10:00:00+05:3',
              '2020-01-01t10:00:00', '2020-01-01\t10:00:00', '2020-01-01  10:00:00', '2020-01-01 10:00:00 ',
              '2020-01-01 10:00:00 +99:99', '2020-01-01 10:00:00-00:00']:
        add(t)
    # Indicators at the start, inside and at the end.
    for c in '-?:,[]{}#&*!|>\'"%@`= \t~<.':
        add(c)
        add(c + 'x')
        add('x' + c)
        add('x' + c + 'y')
        add(c + ' x')
        add('x ' + c)
    # Whitespace, breaks, unicode, controls.
    for s in ['a b', 'a  b', ' a', 'a ', '\ta', 'a\t', 'a\tb', 'a\nb', 'a\n', '\na', 'a\n\nb', 'a \nb',
              'a\n b', 'a\rb', 'a\r\nb', 'a\x85b', 'a\u2028b', 'a\u2029b', '\ufeffa', 'a\ufeff', '\xa0',
              'a\xa0b', 'caf\xe9', '\u65e5\u672c', '\U0001F600', 'a\x00b', 'a\x07b', 'a\x1bb', 'a\x7fb',
              'a\x9fb', '\ud7ff', '\ue000', '\ufffd', '\\', 'a\\b', '---', '...', '--- a', '... a', '----',
              'a---', '- a', '-a', '? a', '?a', ': a', ':a', 'a:', 'a :b', 'a:b', 'a# b', 'a #b', '#a',
              "''", '""', "'", '"', 'null ', ' null']:
        add(s)
    # Long strings: folding at 80 columns.
    words = 'lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor'.split()
    for n in [10, 11, 12, 13, 14, 20, 40]:
        add(' '.join(words[i % len(words)] for i in range(n)))
    add('x' * 79)
    add('x' * 80 + ' ' + 'y' * 10)
    add('a ' * 50)
    add('a  ' * 40 + 'b')
    add(' '.join(['word'] * 30))
    add(' '.join(['w\xe9rd'] * 30))
    add(' '.join(["it's"] * 30))
    add(' '.join(['a: b'] * 30))
    add('x' * 200)
    add('k' * 129)
    rnd = random.Random(14)
    alphabet = 'ab01:-. #\'"\t\n\xe9_xXoO+eE'
    for _ in range(120):
        add(''.join(rnd.choice(alphabet) for _ in range(rnd.randint(1, 12))))
    seen = set()
    uniq = []
    for s in out:
        if s not in seen:
            seen.add(s)
            uniq.append(s)
    return uniq


def scalar_vectors():
    resolver = yaml.resolver.Resolver()
    vecs = []
    for s in scalar_corpus():
        tag = resolver.resolve(ScalarNode, s, (True, False))
        rec = {'s': s, 'tag': tag}
        rec['load_plain'] = load_result('k: ' + s + '\n')
        rec['load_tagged'] = load_result('k: !<' + tag + '> ' + double_quoted(s) + '\n')
        rec['rev'] = [rev_dump(s, ''), rev_dump(s, "'"), rev_dump(s, '"')]
        rec['dump'] = yaml_dump({'k': s})
        vecs.append(rec)
    return vecs


# --- documents -----------------------------------------------------------------------------------------------------

ERROR_DOCS = [
    # scanner
    '\ta: 1\n', '-\tfoo\n', 'key: @x\n', 'key: `x\n', "a: 'x", 'a: "x', 'a: "\\q"\n', 'a: "\\ud800"\n',
    'a: "\\xZZ"\n', '%FOO bar\n---\na: 1\n', '%YAML 1.1\n%YAML 1.1\n---\na\n', '%YAML 2.0\n---\na\n',
    '%TAG ! !foo\n%TAG ! !bar\n---\na\n', 'a: !<>\n', 'a: &\n', 'a: *\n', 'a: |x\n', 'a: |0\n  b\n',
    'a: >\n  b\n c\n', '? a\n? b\n:c', 'a: b\n c: d\n', 'a:\n  - b\n c: d\n', '[a, b',
    '{a: 1', '{a: 1, b}\n', '[a: 1, b: 2: 3]\n', 'a: [1, 2\n', 'repos: [\n', 'a: b: c\n', '- a\nb: c\n',
    'a:\n  b: 1\n   c: 2\n', 'a: "multi\nline', "a: 'x\n\n", 'a: !!int\n', '"a\n', '...\na: [\n',
    'a: 1\n---\nb: 2\n', '---\n  a: 1\n---\nb: 2\n', '--- &x\n  a: 1\n--- [\n', 'a: 1\n...\nb: 2\n',
    # composer
    'a: &x 1\nb: &x 2\n', 'a: *x\n', '&a [&a 1]\n', 'a: &x [1]\nb: *y\n',
    # constructor
    'a: !foo bar\n', 'a: =\n', 'a: <<\n', '? [1, 2]\n: x\n', '? {a: 1}\n: x\n', '{[a]: 1}\n',
    'foo: {<<: 5}\n', 'foo: {<<: [5]}\n', 'repos: []\nfoo: {<<: 5}\n', 'a: {<<: [{x: 1}, 2]}\n',
    'a: !!int x\n', "a: !!int ''\n", "a: !!int '-'\n", "a: !!int '0b'\n", "a: !!int '1::2'\n",
    "a: !!int '0X1F'\n", "a: !!float ''\n", "a: !!float 'abc'\n", 'a: !!bool maybe\n', "a: !!bool 'İ'\n",
    'a: !!timestamp x\n', 'a: 2020-02-30\n', 'a: 2020-13-45\n', 'a: 2020-01-01 25:00:00\n', 'a: 0000-01-01\n',
    'a: 2020-01-01 10:00:00 +24:00\n', 'a: 2020-01-01 10:00:00 -24:00\n', "a: !!binary 'é'\n",
    "a: !!binary 'éé x'\n", "a: !!binary 'aGk'\n", "a: !!binary 'a'\n", 'a: !!omap {a: 1}\n', 'a: !!omap [a]\n',
    'a: !!omap [{a: 1, b: 2}]\n', 'a: !!pairs [[1]]\n', 'a: !!set [a]\n', 'a: !!seq {a: 1}\n', 'a: !!map [1]\n',
    'a: !!str [1]\n', 'a: !!null [1]\n', 'a: !!python/tuple [1]\n', "a: ! '*'\n", 'a: !!timestamp [1]\n',
    'a: [!!int x]\nb: !!int y\n', 'a: [!!int x, !!int z]\n', '- !!int x\n- [!!int y]\n',
    '- [!!int y]\n- !!int x\n', 'a: {b: !!int y}\nc: !!float q\n', 'a: [[!!int x]]\nb: [!!float y]\n',
    'a: 9' + '9' * 4400 + '\n', 'a: !!int "1:' + '9' * 4400 + '"\n', 'a: !!float "' + ':'.join(['1'] * 180) + '"\n',
    # reader
    'a: \x00\n', 'a: 1\nb: \x01\nc: [\n', '\ufeffa: \x02\n', 'a: \x7f\n', 'a: \ufffe\n', 'a: b\x0c\n',
    'x' * 20000 + ': \x03\n', 'a: [\n' + 'x' * 20000 + '\x04\n',
]

VALID_DOCS = [
    '', '# only a comment\n', '---\n', '--- # c\n...\n', 'a: 1\n...\n', '%YAML 1.2\n---\na: yes\n',
    '%YAML 1.1\n---\na: on\n', '%TAG !e! tag:example.com,2000:\n---\na: 1\n', '\ufeffa: 1\n', '\ufeff\ufeffa: 1\n',
    'a: 1\r\nb: 2\r\n', 'a: b #c\n', 'a: b#c\n', 'a:\t1\n', 'a: 1\t# c\n', '{"a":1}\n', '{a:1}\n', '[a:1]\n',
    '[x, ]\n', '{x: 1,}\n', 'a: \x85\n', "a: 'x\n  y'\n", "a: 'x\n\n  y'\n", 'a: x\n  y\n', 'a: "x\\\n  y"\n',
    'a: |\n  x\n  y\n', 'a: |-\n  x\n', 'a: |+\n  x\n\n', 'a: >\n  x\n  y\n\n  z\n', 'a: >2\n   x\n',
    'a: "\\0\\a\\b\\t\\\t\\n\\v\\f\\r\\e\\ \\"\\/\\\\\\N\\_\\L\\P\\x41\\u00e9\\U0001F600"\n',
    "a: 'it''s'\n", 'a: {a: 1, b: 2, a: 3}\n', '{1: a, 1.0: b, true: c}\n', '{.nan: 1, .nan: 2}\n',
    '{0: a, false: b, 0.0: c, -0.0: d}\n', '{null: 1, ~: 2, "": 3}\n', '{2020-01-01: a, "2020-01-01": b}\n',
    'repos: [a]\nrepos: [b]\n', 'c: {<<: [{x: 1}, {x: 2, z: 3}], w: 0}\n',
    'b: &b {x: 1, y: 2}\nc:\n  <<: *b\n  x: 3\n', 'b: &b {y: 1}\nc: {x: 0, <<: *b}\n', 'a: {<<: []}\n',
    'b: &b {<<: {p: 1}, q: 2}\nc: {<<: *b}\n', '&a {<<: *a}\n', '&a {x: 1, <<: *a}\n',
    'a: &a {<<: &b {<<: *a, x: 1}}\n', 'a: &a {k: 1}\nb: {<<: [*a, *a]}\n', '{=: 1}\n', '{=: 1, =: 2}\n',
    'a: !!str {=: x}\n', 'a: !!int {=: "12"}\n', 'x: &x {repo: local, hooks: []}\nrepos: [*x, *x]\n',
    'x: &x {repo: local}\nrepos:\n- <<: *x\n  hooks: []\n', 'a: &a [1, 2]\nb: *a\nc: [*a, *a]\n',
    'a: &a 1\nb: &b *a\n', 'a: ! 123\n', "a: ! '123'\n", "a: ! ''\n", 'a: !\n', 'a: !!str\n', 'a: &x\n',
    'a: !!null x\n', 'a: !!bool On\n', "a: !!int ' 12 '\n", "a: !!int '0o17'\n", "a: !!int '+-5'\n",
    "a: !!int '١٢'\n", "a: !!int '1:-5'\n", "a: !!float ' 1.5 '\n", "a: !!float '1e3'\n", 'a: !!float infinity\n',
    'a: !!float "1:30.5"\n', "a: !!float '-.Inf'\n", "a: !!str 1.10\n", 'a: !!int "0x10"\n',
    "a: !!timestamp '2020-1-1'\n", 'a: !!binary aGk=\n', "a: !!binary 'aGk=x'\n", 'a: !!binary |\n  aGVs\n  bG8=\n',
    'a: !!set {a, b, a}\n', 'a: !!set {1, 1.0, true}\n', 'a: !!omap [{a: 1}, {b: 2}, {a: 3}]\n',
    'a: !!pairs [{a: 1}, {a: 2}]\n', 'a: !!pairs []\n', 'a: !!set {}\n', 'a: !!omap []\n',
    'a: [1, [2, [3, [4]]]]\n', 'a: {b: {c: {d: e}}}\n', '- - - a\n', '-   -   a\n    -   b\n',
    '? |\n  block key\n: v\n', '? [a, b]\n', 'a: 2001-12-14 21:59:43.10 -5\n', 'a: 2002-01-01T00:00:00Z\n',
    'a: 2001-12-14t21:59:43.123456789-05:00\n', 'a: 2020-01-01 10:00:00 +5:30\n', 'a: 2020-01-01 10:00:00.\n',
    'a: 0b_1_0\n', 'a: 0x_1F\n', 'a: 190:20:30\n', 'a: -1:30\n', 'a: 1:30.5\n', 'a: 0700\n', 'a: 1_000\n',
    'a: ' + '9' * 50 + '\n', 'a: -' + '9' * 50 + '\n', 'a: 0x' + 'f' * 50 + '\n', 'a: 0' + '7' * 30 + '\n',
    'a: 1e400\n', 'a: 1.0e+400\n', 'a: 1.0e-400\n', 'a: 6.8523015e+5\n', 'a: 0.1\n', 'a: -0.0\n',
    'k\xe9y: v\xe4lue\n', '"\\u00e9": 1\n', 'a: "\\U0001F600"\n', 'a: \U0001F600\n',
    'a: &x !!str 1\nb: *x\n', 'a: !!seq [1]\nb: !!map {c: 1}\n', 'a: !<tag:yaml.org,2002:int> "7"\n',
    'a: &b [1]\nb: [*b]\n',
    'a: &a [*a]\n', '&a {x: *a}\n', 'a: &a {b: [*a]}\n',
    '? - a\n  - b\n',
    'a:\n- b\n-   c: d\n    e: f\n',
    'a: [' * 20 + ']' * 20 + '\n',
    'a: ' + '{b: ' * 20 + '1' + '}' * 20 + '\n',
]


def limit_doc(kind, n):
    """The documents of the D-SEC-11 limit vectors (the Rust test rebuilds them from `kind` and `n`)."""
    if kind == 'flow_seq':
        return '[' * n + ']' * n + '\n'
    if kind == 'flow_map':
        return '{a: ' * n + '1' + '}' * n + '\n'
    if kind == 'block_seq':
        return '- ' * n + 'x\n'
    if kind == 'bomb':
        lines = ['a0: &a0 [' + ', '.join(['lol'] * 10) + ']']
        for i in range(1, n):
            lines.append('a%d: &a%d [%s]' % (i, i, ', '.join(['*a%d' % (i - 1)] * 10)))
        return '\n'.join(lines) + '\n'
    raise ValueError(kind)


def load_vectors():
    vecs = []
    for d in ERROR_DOCS + VALID_DOCS:
        vecs.append({'yaml': d, 'result': load_result(d)})
    # Random flow documents built from a small grammar (anchors, tags, merges, quoting).
    rnd = random.Random(1462)
    atoms = ['a', 'b', '1', '1.5', 'yes', 'null', '~', '', "'q'", '"d"', '0x1F', '2020-01-01', '=', '!!str 3',
             '!!int "4"', '1:30', '-', 'é', '"\\t"', '.inf']

    def gen(depth, anchors):
        r = rnd.random()
        if depth > 3 or r < 0.4:
            a = rnd.choice(atoms)
            if anchors and rnd.random() < 0.15:
                return '*' + rnd.choice(anchors)
            return a
        if r < 0.7:
            items = [gen(depth + 1, anchors) for _ in range(rnd.randint(0, 3))]
            text = '[' + ', '.join(items) + ']'
        else:
            pairs = []
            for _ in range(rnd.randint(0, 3)):
                k = rnd.choice(['a', 'b', 'c', '1', '1.0', 'true', '<<', '=', '? [x]'])
                v = gen(depth + 1, anchors)
                if k == '<<' and rnd.random() < 0.7 and anchors:
                    v = '*' + rnd.choice(anchors)
                pairs.append(k + ': ' + v)
            text = '{' + ', '.join(pairs) + '}'
        if rnd.random() < 0.3:
            name = 'n%d' % len(anchors)
            anchors.append(name)
            text = '&' + name + ' ' + text
        return text

    for _ in range(300):
        anchors = []
        doc = 'r: ' + gen(0, anchors) + '\n'
        vecs.append({'yaml': doc, 'result': load_result(doc)})
    return vecs


def limit_vectors():
    """What PyYAML does with documents beyond snp's limits: it loads them (D-SEC-11 evidence)."""
    out = []
    for kind, n in [('flow_seq', 512), ('flow_seq', 513), ('flow_seq', 10000), ('flow_map', 512),
                    ('flow_map', 513), ('block_seq', 600), ('bomb', 10)]:
        try:
            v = yaml.load(limit_doc(kind, n), Loader=LOADER)
            outcome = 'loaded: %d nodes when expanded' % expanded_size(v, 10**12)
        except Exception as e:  # noqa: BLE001
            outcome = 'error: %s: %s' % (type(e).__name__, str(e).splitlines()[0])
        out.append({'kind': kind, 'n': n, 'python': outcome})
    return out


# --- compose -------------------------------------------------------------------------------------------------------

def compose_vectors():
    out = []
    docs = [
        'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        stages: [commit, push]\n',
        'a: &s [commit]\nb: *s\nc: "q" # c\nd: |\n  lit\n',
        '\ufeffk\xe9: v\xe9\nx: [1, {y: 2}]\n',
        'a: !!str 1\nb: !foo {x: 1}\nc: ! 12\nd: !!python/tuple [1]\n',
        '---\n- a\n- - b\n  - c\n- {d: e}\n...\n',
        "sha: v1.2.0\n'sha': 'x'\n\"sha\": \"y\"\n",
        'a:\n  - >-\n    folded\n    text\n  - plain\n    multi\n',
        '',
        '# nothing\n',
        'x: \u65e5\u672c\u8a9e\ny: [\U0001F600, e\u0301]\n',
        # End marks of events that do not consume their token (indentless sequences, implicit flow mappings).
        'a:\n- x\n? b\n: c\n',
        'a:\n- x\nb: c\n',
        '[a: b, c]\n',
        '- [a: b]\n- [? x : y, z]\n',
        '{a: [b, c], d: {e: f}}\n',
        'a: |+\n  x\n\nb: >-\n  y\n',
        'a: !!binary aGk=\nb: &x !!set {p}\nc: *x\n',
        '\r\nk:\r\n  - v\r\n',
    ]
    for d in docs:
        try:
            root = yaml.compose(d, Loader=LOADER)
        except Exception as e:  # noqa: BLE001
            out.append({'yaml': d, 'error': [type(e).__name__, str(e)]})
            continue
        nodes = []
        ids = {}

        def visit(n):
            if id(n) in ids:
                return ids[id(n)]
            i = len(nodes)
            ids[id(n)] = i
            rec = {'tag': n.tag, 'start': [n.start_mark.index, n.start_mark.line, n.start_mark.column],
                   'end': n.end_mark.index}
            nodes.append(rec)
            if isinstance(n, yaml.ScalarNode):
                rec['scalar'] = n.value
                rec['style'] = n.style
            elif isinstance(n, yaml.SequenceNode):
                rec['seq'] = [visit(c) for c in n.value]
            else:
                rec['map'] = [[visit(k), visit(v)] for k, v in n.value]
            return i

        if root is not None:
            visit(root)
        out.append({'yaml': d, 'nodes': nodes})
    return out


# --- dump ----------------------------------------------------------------------------------------------------------

AUTOUPDATE_REDUMP_SRC = (
    '# keep me\n'
    'repos: [{repo: /path/hooks1, rev: v1.1.0, hooks: [{id: h1}]},  # c\n'
    '        {repo: local, hooks: [{id: x, name: x, entry: x, language: system}]}]\n'
)

DUMP_SRCS = [
    AUTOUPDATE_REDUMP_SRC,
    'repos:\n- repo: x\n  rev: v1\n  hooks:\n  - id: a\n    args: [--x, y]\n    stages: []\nfail_fast: true\nn: null\n',
    'repos:\n- repo: /tmp/x y\n  rev: "1.0"\n  hooks:\n  - id: flake8\n  - id: "\\xE9"\n    args: [--max=1, "yes", ""]\n'
    '    always_run: true\n    n: 3\n',
    '{}\n', '[]\n', 'a: {}\nb: []\nc: [[], {}]\n', 'a: 1\n', '"x"\n', '1\n', '~\n', '[1, [2, [3]], {a: [b]}]\n',
    'a: [[1, 2], [3]]\n',
    'k: !!binary aGVsbG8=\n', 'k: !!set {a, b}\n', 's: !!set {}\n', 'k: !!omap [{a: 1}]\n', 'k: !!pairs [{a: 1}, {a: 2}]\n',
    'd: 2020-01-01\nt: 2001-12-14 21:59:43.10 -5\nu: 2002-01-01T00:00:00Z\nn: 2020-01-01 10:00:00\n',
    'f: [1.5, -0.0, .inf, -.inf, .nan, 1.0e+16, 1.0e-5, 6.8523015e+5, 100000000000000000000.0, 0.1]\n',
    'i: [0, -1, 9223372036854775808, -' + '9' * 40 + ']\n', 'b: [yes, no, on, off]\n',
    'long: ' + ' '.join(['word'] * 40) + '\n', 'long2: "' + ' '.join(['w\\xe9rd'] * 30) + '"\n',
    'ml: "line1\\nline2\\n\\n"\n', 'ml2: "a\\n b"\n', 'ml3: "a \\nb"\n', 'trail: "a "\n', 'lead: " a"\n',
    'tab: "a\\tb"\n', 'cr: "a\\rb"\n', 'nel: "a\\x85b"\n', 'ls: "a\\u2028b"\n', 'emoji: "\\U0001F600 x"\n',
    'ctrl: "\\x01\\x7f\\x9f"\n', 'bom: "\\ufeffa"\n', 'nul: "a\\0b"\n', 'bs: "a\\\\b"\n', 'dq: "a\\"b"\n',
    'sq: "it\'s"\n', 'both: "x\\"y\'z"\n', '"key with spaces": v\n', '"k: v": 1\n', '"": empty key\n',
    '? "' + 'k' * 130 + '"\n: long key\n', '? "multi\\nline key"\n: v\n', '1: int key\n2.5: float key\nnull: n\ntrue: t\n',
    '"---": a\n"...": b\nc: "---"\nd: "- x"\ne: "-x"\nf: "? x"\ng: ":x"\n',
    'x: "' + 'a ' * 60 + '"\n', 'x: "' + 'a  ' * 40 + 'b"\n', 'x: "' + 'é ' * 50 + '"\n',
    'x: [' + ', '.join('"%s"' % ('w' * 30) for _ in range(5)) + ']\n',
    'nested:\n  a:\n    b:\n      c: [1, {d: [e, {f: g}]}]\n',
    # Shared objects: PyYAML anchors them (`&id001`) and aliases later uses (`*id001`).
    'x: &x {repo: local, hooks: []}\nrepos: [*x, *x]\n',
    'd: &d 2020-01-01\ne: *d\nf: [*d, 2020-01-01]\n',
    't: &t 2001-12-14 21:59:43.10 -5\nu: {*t : x}\n',
    'b: &b {x: 1, y: [1, 2]}\nc:\n  <<: *b\n  x: 3\nd: {<<: *b}\n',
    'l: &l []\nm: &m {}\nn: [*l, *m, *l]\n',
    's: &s !!set {a, b}\nt: [*s, *s]\n',
    'p: &p !!pairs [{a: 1}, {a: 2}]\nq: *p\n',
    'a: &a [&b [x], *b]\nc: *a\n',
    'k: &k 1\nm: {*k : v, w: *k}\n',
    'big: &big {' + ', '.join('k%d: v' % i for i in range(5)) + '}\nr: [*big, *big]\n',
]


def dump_vectors():
    out = []
    for src in DUMP_SRCS:
        v = yaml.load(src, Loader=LOADER)
        out.append({'yaml': src, 'dump': yaml_dump(v)})
    rnd = random.Random(52)
    for _ in range(150):
        s = ''.join(rnd.choice('ab :#-\'"\n\t\xe9\u2028x01.') for _ in range(rnd.randint(0, 100)))
        doc = 'k: ' + double_quoted(s) + '\n'
        out.append({'yaml': doc, 'dump': yaml_dump(yaml.load(doc, Loader=LOADER))})
    return out


# --- decode --------------------------------------------------------------------------------------------------------

DECODE_BYTES = [
    b'', b'ok', b'\xff', b'\x80', b'a\xc3', b'\xc3(', b'\xe2\x82', b'\xe2(\xa1', b'\xe2\x82(', b'\xf0\x9f\x98',
    b'\xf0(\x8c(', b'\xc0\xaf', b'\xc1\xbf', b'\xe0\x80\xaf', b'\xe0\xa0', b'\xed\xa0\x80', b'\xf4\x90\x80\x80',
    b'\xf5\x80', b'\xf8\x88\x80\x80\x80', b'ok\xffok', b'abc\xe2\x82', b'\xe2\x82abc', b'\xf0\x9f\x98\x80\xff',
    b'caf\xe9.txt\x00', b'\xef\xbb\xbfbom', b'a\r\nb\rc\n', b'\r\r\n\n', b'\xef\xbb\xbfa: 1\r\n',
    b'repos: []\n\xe9', b'x' * 9000 + b'\xff', b'\xff\xfea\x00', b'a\x00\r',
]


def decode_vectors():
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, 'f')
        for b in DECODE_BYTES:
            with open(path, 'wb') as f:
                f.write(b)
            try:
                with open(path, encoding='utf-8') as f:
                    out.append({'hex': b.hex(), 'text': f.read()})
            except UnicodeDecodeError as e:
                out.append({'hex': b.hex(), 'error': str(e)})
    return out


# --- random documents (differential corpus) ------------------------------------------------------------------------

def random_vectors():
    """Random block/flow documents, mutated documents, block scalars, odd line breaks, BOMs, tags, long keys and
    inputs beyond libyaml's 16 KiB reader chunk: the load outcome and yaml_dump(yaml_load(text))."""
    out = []
    rnd = random.Random(7)
    chars = list('abcxyz019 -_:#,[]{}&*!|>\'"%@`?=~<.\t\n\\') + ['\xe9', ' ', '\x85', '﻿', '\x00',
                                                                '\x07', '\U0001F600', '\xa0', '\r']
    atoms = ['a', 'b', '1', '1.5', 'yes', 'No', 'null', '~', '', "'q'", '"d"', '0x1F', '2020-01-01', '=', '!!str 3',
             '!!int "4"', '1:30', '-', 'é', '"\\t"', '.inf', '0777', '1e3', '!!binary aGk=', '!!float 1', '<<',
             '"a\\nb"', '|\n  lit\n', '>\n  fold\n', 'x y z', '2001-12-14 21:59:43.10 -5', '0b1', '-0', '+.5', '.5']

    def rand_text(n):
        return ''.join(rnd.choice(chars) for _ in range(n))

    def gen_block(depth, indent, anchors):
        r = rnd.random()
        pad = ' ' * indent
        if depth > 3 or r < 0.3:
            a = rnd.choice(atoms)
            if anchors and rnd.random() < 0.1:
                return ' *' + rnd.choice(anchors)
            if '\n' in a:
                return ' ' + a.replace('\n  ', '\n' + pad + '  ')
            return ' ' + a
        anchor = ''
        if rnd.random() < 0.2:
            name = 'n%d' % len(anchors)
            anchors.append(name)
            anchor = ' &' + name
        if r < 0.6:
            lines = [pad + '-' + gen_block(depth + 1, indent + 2, anchors) for _ in range(rnd.randint(1, 3))]
            return anchor + '\n' + '\n'.join(lines)
        lines = []
        for _ in range(rnd.randint(1, 3)):
            k = rnd.choice(['a', 'b', 'c', '1', 'true', '<<', '=', '"q k"', '? x'])
            if k == '? x':
                lines.append(pad + '? x\n' + pad + ':' + gen_block(depth + 1, indent + 2, anchors))
            else:
                v = gen_block(depth + 1, indent + 2, anchors)
                if k == '<<' and anchors and rnd.random() < 0.6:
                    v = ' *' + rnd.choice(anchors)
                lines.append(pad + k + ':' + v)
        return anchor + '\n' + '\n'.join(lines)

    words = ['lorem', 'ipsum', 'a', 'é', '日本', '\U0001F600', 'x:y', 'k #c', "it's", '"q"', '-', '?', '!t', '&a',
             '*a', '%', '@', '`', '[', ']', '{', '}', ',', '...', '---', '\t', '\\', '\x85', ' ', 'yes', '1.5',
             '0x1F', '~']

    def rand_words(n):
        return ' '.join(rnd.choice(words) for _ in range(n))

    def block_scalar(indent):
        ind = rnd.choice(['', '1', '2', '4', '9'])
        chomp = rnd.choice(['', '-', '+'])
        head = rnd.choice(['|', '>']) + rnd.choice([ind + chomp, chomp + ind])
        pad = ' ' * (indent + 2)
        lines = []
        for _ in range(rnd.randint(0, 4)):
            r = rnd.random()
            if r < 0.2:
                lines.append('')
            elif r < 0.3:
                lines.append(pad + '  ' + rand_words(rnd.randint(1, 3)))
            else:
                lines.append(pad + rand_words(rnd.randint(1, 8)))
        return head + rnd.choice([' # c', '']) + '\n' + '\n'.join(lines) + '\n' * rnd.randint(0, 3)

    def scalar2():
        r = rnd.random()
        if r < 0.25:
            return block_scalar(0)
        if r < 0.45:
            return double_quoted(rand_words(rnd.randint(0, 25)))
        if r < 0.55:
            return "'" + rand_words(rnd.randint(0, 10)).replace("'", "''") + "'"
        if r < 0.65:
            return '!e%C3%A9 x' if rnd.random() < 0.5 else '!<tag:yaml.org,2002:str> 5'
        return rand_words(rnd.randint(1, 4)).replace('\n', ' ')

    def doc2():
        lines = []
        if rnd.random() < 0.1:
            lines += ['%YAML 1.1', '%TAG !e! tag:example.com,2000:', '---']
        for i in range(rnd.randint(1, 6)):
            key = rnd.choice(['a', 'b%d' % i, 'k' * rnd.randint(120, 140), '? ' + rand_words(2),
                              '"' + 'é' * rnd.randint(300, 600) + '"'])
            if key.startswith('? '):
                lines.append(key)
                lines.append(': ' + scalar2())
            else:
                lines.append(key + ': ' + scalar2())
        text = '\n'.join(lines) + '\n'
        if rnd.random() < 0.3:
            text = text.replace('\n', rnd.choice(['\n', '\r\n', '\r', '\x85', ' ']))
        if rnd.random() < 0.1:
            text = '﻿' + text
        if rnd.random() < 0.05:
            pos = rnd.randint(0, len(text))
            text = text[:pos] + rnd.choice(['\t', '\x01', '\x7f', '￾', '\x00']) + text[pos:]
        return text

    def big_doc():
        filler = ''.join('k%d: %s\n' % (i, 'x' * 60) for i in range(rnd.randint(200, 600)))
        size = len(filler.encode('utf-8'))
        pos = max(0, min(rnd.choice([16380, 16383, 16384, 16385, 16390, 32766, 32768, size - 3]), len(filler)))
        err_pos = rnd.randint(0, pos)
        text = (filler[:err_pos] + rnd.choice(['', 'a: b: c\n', '[', '\t']) + filler[err_pos:pos]
                + rnd.choice(['\x01', 'é\x02', '\U0001F600\x03']) + filler[pos:])
        return '﻿' + text if rnd.random() < 0.5 else text

    docs = []
    for _ in range(1500):
        kind = rnd.random()
        if kind < 0.35:
            docs.append('r:' + gen_block(0, 2, []) + '\n')
        elif kind < 0.6:
            docs.append(rand_text(rnd.randint(0, 30)))
        elif kind < 0.8:
            base = 'r:' + gen_block(0, 2, []) + '\n'
            pos = rnd.randint(0, len(base))
            docs.append(base[:pos] + rand_text(rnd.randint(1, 3)) + base[pos:])
        else:
            docs.append('k: ' + double_quoted(rand_text(rnd.randint(0, 120))) + '\n')
    docs += [doc2() for _ in range(400)]
    docs += [big_doc() for _ in range(6)]
    for text in docs:
        rec = {'yaml': text, 'result': load_result(text)}
        if 'value' in rec['result']:
            rec['redump'] = yaml_dump(yaml.load(text, Loader=LOADER))
        out.append(rec)
    return out


def write(out_dir, name, data, compact=False):
    path = os.path.join(out_dir, name)
    with open(path, 'w', encoding='utf-8', newline='\n') as f:
        if compact:
            # One vector per line, UTF-8 kept: the large corpus stays small and diffs stay readable.
            f.write('{"meta": %s, "vectors": [\n' % json.dumps(data['meta'], sort_keys=True))
            vecs = data['vectors']
            for i, v in enumerate(vecs):
                f.write(json.dumps(v, ensure_ascii=False, sort_keys=True))
                f.write(',\n' if i + 1 < len(vecs) else '\n')
            f.write(']}\n')
        else:
            json.dump(data, f, ensure_ascii=True, indent=1, sort_keys=True)
            f.write('\n')


def main():
    out_dir = sys.argv[1]
    os.makedirs(out_dir, exist_ok=True)
    meta = {'python': sys.version.split()[0], 'pyyaml': yaml.__version__}
    import _yaml
    meta['libyaml'] = _yaml.get_version_string()
    write(out_dir, 'scalars.json', {'meta': meta, 'vectors': scalar_vectors()})
    write(out_dir, 'load.json', {'meta': meta, 'vectors': load_vectors()})
    write(out_dir, 'limits.json', {'meta': meta, 'vectors': limit_vectors()})
    write(out_dir, 'compose.json', {'meta': meta, 'vectors': compose_vectors()})
    write(out_dir, 'dump.json', {'meta': meta, 'vectors': dump_vectors()})
    write(out_dir, 'decode.json', {'meta': meta, 'vectors': decode_vectors()})
    write(out_dir, 'random.json', {'meta': meta, 'vectors': random_vectors()}, compact=True)


if __name__ == '__main__':
    sys.setrecursionlimit(20000)
    main()
