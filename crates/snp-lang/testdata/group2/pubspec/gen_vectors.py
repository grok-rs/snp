"""Generates vectors.jsonl: what pre-commit 4.7.0's dart `_install_dir` sees in `pubspec_contents['executables']`.

Run with the pre-commit 4.7.0 oracle venv (PyYAML 6.0.3 with LibYAML, `CSafeLoader`), isolated:

    target/oracle/pc470-py312/bin/python -I crates/snp-lang/testdata/group2/pubspec/gen_vectors.py \
        crates/snp-lang/testdata/group2/pubspec/vectors.jsonl [random-count]

Each line is {"src": <pubspec text>, "want": <outcome>, "accept": <bool>}; `accept` marks realistic pubspecs the snp
reader must read exactly (elsewhere it may refuse valid YAML, never accept invalid YAML or read it differently).
Outcomes: {"error": <exception type>} (yaml_load raised), {"empty": true} (document is None), {"notmap": <type>},
{"missing": true}, {"null": true}, {"notiter": <type>}, {"names": [...], "nonstr": <type or null>}.

Owner: WP-25.
"""
from __future__ import annotations

import json
import os
import random
import sys
import tempfile

from pre_commit.yaml import yaml_load


def expect(src: str) -> dict:
    fd, path = tempfile.mkstemp(suffix='.yaml')
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(src.encode('utf-8', 'surrogatepass'))
        # `_install_dir`: `with open(prefix_p.path('pubspec.yaml')) as f: yaml_load(f)` (text mode).
        try:
            with open(path, encoding='UTF-8') as f:
                doc = yaml_load(f)
        except Exception as e:  # noqa: BLE001 (any load failure is pre-commit's crash)
            return {'error': type(e).__name__}
    finally:
        os.unlink(path)
    if doc is None:
        return {'empty': True}
    if not isinstance(doc, dict):
        return {'notmap': type(doc).__name__}
    if 'executables' not in doc:
        return {'missing': True}
    ex = doc['executables']
    if ex is None:
        return {'null': True}
    if not isinstance(ex, (dict, list, str)):
        return {'notiter': type(ex).__name__}
    names: list[str] = []
    nonstr = None
    for item in ex:
        if not isinstance(item, str):
            nonstr = type(item).__name__
            break
        try:
            item.encode('utf-8')
        except UnicodeEncodeError:
            return {'error': 'UnicodeEncodeError'}
        names.append(item)
    return {'names': names, 'nonstr': nonstr}


# Realistic pubspecs: the reader must accept them and agree.
REALISTIC = [
    "name: pre_commit_empty_pubspec\nenvironment:\n  sdk: '>=2.12.0'\nexecutables: {}\n",
    "name: hello\ndescription: A sample command-line application.\nversion: 1.0.0\n"
    "homepage: https://example.com\n\nenvironment:\n  sdk: '>=2.12.0 <3.0.0'\n\n"
    "dependencies:\n  args: ^2.0.0\n  path: ^1.8.0\n\ndev_dependencies:\n  lints: ^1.0.0\n  test: ^1.16.0\n\n"
    "executables:\n  hello:\n  hi: hello\n",
    "name: tool\ndescription: >-\n  A long description that is folded\n  over several lines.\n"
    "executables:\n  tool:\n",
    "name: tool\ndescription: |\n  Literal text: with colons\n  - and dashes\n\n  # and hashes\nexecutables:\n  tool:\n",
    "name: tool\ndescription: A plain description\n  continued on the next line.\nexecutables:\n  a:\n  b:\n",
    "name: x\ndependencies:\n  foo:\n    git:\n      url: https://github.com/x/foo.git\n      ref: main\n"
    "  bar:\n    path: ../bar\n  baz:\n    hosted: https://pub.example.com\n    version: ^1.0.0\n"
    "executables:\n  foo: # the main tool\n  bar: bar_main\n",
    "# leading comment\nname: x # trailing comment\nexecutables:\n  # comment line\n  'quoted':\n  \"dq\":\n",
    "name: x\nflutter:\n  assets:\n    - images/a.png\n    - images/b.png\n  fonts:\n    - family: Foo\n"
    "      fonts:\n        - asset: fonts/Foo.ttf\n          weight: 700\nexecutables:\n  x:\n",
    "name: x\nexecutables:\n- a\n- b\n",
    "name: x\nexecutables: [a, b]\n",
    "name: x\nexecutables: {a: b, c}\n",
    "name: x\nexecutables: {\n  a: b,\n  c: d,\n}\n",
    "name: x\r\nexecutables:\r\n  a:\r\n  b:\r\n",
    "\ufeffname: x\nexecutables:\n  a:\n",
    "---\nname: x\nexecutables:\n  a:\n...\n",
    "name: x\nversion: 1.2.3+4\nexecutables:\n  a.b:\n  c-d:\n  e_f:\n",
    "name: x\nplatforms:\n  linux:\n  macos:\nfalse_secrets:\n  - /lib/a.dart\nexecutables:\n  a:\n",
    "name: x\nscreenshots:\n  - description: 'A screenshot'\n    path: s.png\nexecutables:\n  a:\n",
    "name: x\ntopics: [cli, tool]\nexecutables:\n  a:\n",
    "name: x\nexecutables:\n  dup:\n  other:\n  dup: again\n",
    "name: x\nexecutables:\n",
    "name: x\n",
    "name: x\nexecutables: ~\n",
    "name: x\nexecutables: ''\n",
    "name: x\nexecutables: tool\n",
    "name: x\nexecutables:\n  123:\n",
    "name: x\nexecutables:\n  a:\n  yes:\n",
    "name: x\nexecutables: 5\n",
    "name: x\nexecutables:\n  {a: b}\n",
    "name: x\nexecutables:\n  {a: b, c: d}\n",
    "name: x\nexecutables:\n  - a\n  - {b: c}\n",
]

# Hand-written edge cases (several from the WP-25 review differential).
HAND = [
    "executables:\n  foo:\n  bar: baz\n",
    "name: a\ndescription: |\n  hello\n  \tworld\nexecutables:\n  foo:\n",
    "name: a\ndescription: >-\n  hello: world\n  - x\nexecutables:\n  foo:\n",
    "executables: {a: b, c}\n", "executables: [a, b]\n", "executables:\n- a\n- b\n", "executables:\n  - a\n  - b\n",
    "executables: foo\n", "executables: ~\n", "executables:\n", "executables: {}\n", "executables:\n  123:\n",
    "executables:\n  yes:\n", "executables:\n  'a b':\n", "executables:\n  \"x\\ty\":\n",
    "executables:\n  foo: # c\n  # comment\n  bar:\n",
    "executables:\n  foo:\nexecutables:\n  bar:\n", "executables:\n  foo:\n    bar\n", "executables:\n  a:b:\n",
    "executables:\n  a :\n", "executables:\n  null:\n", "executables:\n  ~:\n", "executables:\n  true:\n",
    "executables:\n  1.5:\n", "executables:\n  0x1f:\n", "executables:\n  foo: bar\n  foo: baz\n",
    "executables:\n  -foo:\n", "executables:\n  foo#bar:\n", "executables:\n  'it''s':\n",
    "%YAML 1.1\n---\nexecutables:\n  foo:\n", "---\nexecutables:\n  foo:\n...\n", "executables:\n  foo:\n---\nx: 1\n",
    "executables:\n    foo:\n    bar:\n", "executables:\n  foo:\n bar:\n", "executables: [a,\n  b]\n",
    "executables: {a: [1, 2]}\n", "x: &a 1\nexecutables:\n  foo:\n", "x: !!str 1\nexecutables:\n  foo:\n",
    "description: \"multi\n  line: x\"\nexecutables:\n  foo:\n", "description: 'multi\n# not comment'\nexecutables:\n  foo:\n",
    "description: plain\n  continued: no\nexecutables:\n  foo:\n", "executables:\n\tfoo:\n", "executables:\n  foo:\t\n",
    "executables:\n  foo: \"a: b\"\n", "executables:\n  ? foo\n", "executables:\n  \"foo\" : bar\n",
    "executables:\n  - 'a'\n  - \"b\"\n", "executables:\n  - a: b\n", "executables:\n  - [a]\n", "executables: [ ]\n",
    "executables: {a, b, }\n", "executables: [a, b, ]\n", "executables: !!null\n", "executables: null # c\n",
    "executables: Null\n", "executables: NULL\n", "executables: nUll\n", "executables: \"\"\n",
    "executables:\n  \u00dcn\u00efc\u00f6d\u00e9:\n", "executables:\n  foo\n", "executables:\n  foo:bar\n",
    "a:\n- 1\n- 2\nexecutables:\n- x\n", "executables:\n  foo:\n# top comment\n  bar:\n",
    "executables:\n  foo:\n\n  bar:\n", "executables:   \n  foo:\n", "executables :\n  foo:\n",
    "'executables':\n  foo:\n", "\"executables\":\n  foo:\n", "executables:\n  @foo:\n", "executables:\n  %foo:\n",
    "executables:\n  `foo:\n", "executables:\n  foo bar:\n", "executables:\n  'foo'bar:\n",
    "executables:\n  foo: |\n    x\n", "executables: {a: b,\n  c: d}\n", "executables:\n  =:\n",
    "executables:\n  <<: {a: b}\n", "executables:\n  foo: *x\n", "executables:\n  \"a\\x41\":\n",
    "executables:\n  \"a\\u00e9\":\n", "executables:\n  \"a\\e\":\n", "executables:\n  \"a\\N\":\n",
    "executables:\n  \"a\\_\":\n", "executables:\n  \"a\\\nb\":\n", "executables:\n  ':':\n",
    "executables:\n  [a, b]\n", "description: |\n  foo\n# comment\n  bar\nexecutables:\n  x:\n",
    "executables:\n  a: b # c: d\n", "executables:\n  a:   \n  b :\n", "executables:\n  a: 'x\n    y'\n",
    "executables:\n  \"a\": \n", "executables:\n  a: {b: c}\n", "executables:\n  a: [b, c]\n",
    "executables:\n  a: -b\n", "executables:\n  - a # c\n", "executables:\n  -  a\n",
    "executables:\n  a: \"b\"\nname: x\n", "executables: {\"a\": b}\n", "executables: {'a': b}\n",
    "executables: {a: 'b,c'}\n", "executables: {a: \"x}\"}\n", "executables: [\"a,b\"]\n", "executables:\n  a#b:\n",
    "executables:\n  a #b:\n", "name: 'x' # c\nexecutables:\n  a:\n", "name: \"x#y\"\nexecutables:\n  a:\n",
    "x: 'a''b'\nexecutables:\n  a:\n", "x: \"a\\\"b\"\nexecutables:\n  a:\n",
    "executables: [a, b]\n- a\n", "executables:\n  x: y: z\n", "executables:\n  : x\n",
    "executables:\n odd:\nname: x\n  \"d\":\n", "executables:\n  a:\n  a: b\n", "executables: {a: b, a: c}\n",
    "executables:\n  \"a\\/b\":\n", "executables:\n  0b_:\n", "executables:\n  0x_:\n", "executables:\n  0b2:\n",
    "executables:\n  1_000:\n", "executables:\n  1:30:\n", "executables:\n  1:60:\n", "executables:\n  1:30.5:\n",
    "executables:\n  .inf:\n", "executables:\n  -.inf:\n", "executables:\n  .nan:\n", "executables:\n  .NaN:\n",
    "executables:\n  1e5:\n", "executables:\n  1.e+5:\n", "executables:\n  .5:\n", "executables:\n  +1:\n",
    "executables:\n  0o17:\n", "executables:\n  017:\n", "executables:\n  08:\n", "executables:\n  2to3:\n",
    "executables:\n  On:\n", "executables:\n  oN:\n", "executables:\n  y:\n", "executables:\n  n:\n",
    "executables:\n  2020-01-01:\n", "x: 2020-13-45\nexecutables:\n  a:\n", "x: =\nexecutables:\n  a:\n",
    "executables: true\n", "executables: 1.5\n", "executables: 'abc'\n", "executables: \"a/b\"\n",
    "executables:\n  - a\n  -\n", "executables:\n  - a\n  - ~\n", "executables:\n  - [b]\n", "executables:\n  - 1\n",
    "executables:\n- a\nname: x\n", "executables:\n- a\n - b\n", "executables:\n- a\n  - b\n",
    "executables:\n  - a\n- b\n", "a:\n  b:\n    c: d\n  e: f\nexecutables:\n  a:\n",
    "a:\n    b: 1\n  c: 2\nexecutables:\n  a:\n", "a: b\n  c\nexecutables:\n  a:\n", "a: b\n\n  c\nexecutables:\n  a:\n",
    "a: b\n  - c\nexecutables:\n  a:\n", "a: b\n  c: d\nexecutables:\n  a:\n", "a: |\nexecutables:\n  a:\n",
    "a: |\n\n    x\n  y\nexecutables:\n  a:\n", "a: |\n      \n  x\nexecutables:\n  a:\n", "a: |2\n   x\nexecutables:\n  a:\n",
    "a: |-\n  x\nexecutables:\n  a:\n", "a: >+\n  x\n\nexecutables:\n  a:\n", "a: | x\nexecutables:\n  a:\n",
    "a: [b, {c: d}]\nexecutables:\n  a:\n", "a: {b: [c]}\nexecutables:\n  a:\n", "a: [b\nexecutables:\n  a:\n",
    "a: [b]]\nexecutables:\n  a:\n", "a: [b] c\nexecutables:\n  a:\n", "a: [b, c\nd]\nexecutables:\n  a:\n",
    "a: [b,\n\n  c]\nexecutables:\n  a:\n", "a: [b, # c\n  d]\nexecutables:\n  a:\n", "a: {b: c d}\nexecutables:\n  a:\n",
    "a: {b c: d}\nexecutables:\n  a:\n", "a: [b c]\nexecutables:\n  a:\n", "a: [b, , c]\nexecutables:\n  a:\n",
    "executables: {a: , b: }\n", "executables: {a:}\n", "executables: {a:b}\n", "executables: {? a}\n",
    "executables: [a: b]\n", "executables: [a, [b]]\n", "executables: {a: b}  # c\n", "executables: {a: b} c\n",
    "executables:\n  {a: b}\n  c: d\n", "executables: [a#b]\n", "executables: [a #b\n  ]\n", "executables: [a]\n...\n",
    "executables: [a]\n...\n# c\n", "executables: [a]\n...\nb: 1\n", "executables: [a]\n---\n", "--- # c\nexecutables: [a]\n",
    "--- executables: [a]\n", "---\n---\nexecutables: [a]\n", "# only a comment\n", "", "---\n", "...\n",
    "- a\n", "foo\n", "'foo'\n", "[a]\n", "{executables: [a]}\n", "  executables: [a]\n", "executables: [a]\nfoo\n",
    "executables:\n  a:\n  b:\n    c\n", "executables:\n  a: 'b'c\n", "executables:\n  a: \"b\" # c\n",
    "executables:\n  a: \"b\"#c\n", "executables:\n  a: b\n  # c\n    d\n", "executables:\n  a: b\n\n\n  c: d\n",
    "executables:\n  a\n  b\n", "executables:\n  a\n\n  b\n", "executables:\n  a b\n", "executables:\n  ab # c\n",
    "executables: a b\n", "executables:\n  'a\n  b'\n", "executables:\n  - a\n    b\n", "executables:\n  - a: b\n    c: d\n",
    "executables:\n  - a: b\n   c: d\n", "executables:\n  -\n    a\n", "executables:\n  -\n  a\n", "executables:\n  - - a\n",
    "executables:\n  a: b\n  -\n", "executables:\n  a:\n  - b\n", "executables:\n  a:\n  - b\n  c:\n",
    "executables:\n  a:\n    - b\n  c:\n", "x: \u0085\nexecutables:\n  a:\n", "x: \u2028\nexecutables:\n  a:\n",
    "x: \x01\nexecutables:\n  a:\n", "x: \x7f\nexecutables:\n  a:\n", "x: \ufeff\nexecutables:\n  a:\n",
    "x: \ufffe\nexecutables:\n  a:\n", "x: \U0001f600\nexecutables:\n  a:\n", "executables:\n  \"\\ud800\":\n",
    "executables:\n  \"\\U0001f600\":\n", "executables:\n  \"\\x\":\n", "executables:\n  \"\\q\":\n",
    "executables:\r  a:\r", "executables:\n  a:\r\n  b:\n", "a: 'x'\"\nexecutables:\n  a:\n", "a: \"x\"'\nexecutables:\n  a:\n",
    "a: x, y\nexecutables:\n  a:\n", "a: x [y]\nexecutables:\n  a:\n", "a: x]y\nexecutables:\n  a:\n",
    "a: x {y\nexecutables:\n  a:\n", "a: x: \nexecutables:\n  a:\n", "a: http://x:80/y\nexecutables:\n  a:\n",
    "a: x # y: z\nexecutables:\n  a:\n", "a: x#y: z\nexecutables:\n  a:\n", "a: '#'\nexecutables:\n  a:\n",
    "a: x ' y # z\nexecutables:\n  a:\n", "a b: c\nexecutables:\n  a:\n", "a[b]: c\nexecutables:\n  a:\n",
    "a{b: c\nexecutables:\n  a:\n", "a,b: c\nexecutables:\n  a:\n", "a?: c\nexecutables:\n  a:\n", "a!b: c\nexecutables:\n  a:\n",
    "a&b: c\nexecutables:\n  a:\n", "a*b: c\nexecutables:\n  a:\n", "'a' 'b': c\nexecutables:\n  a:\n",
    "a: 'b' : c\nexecutables:\n  a:\n", "a: b\n---\n", "a: b\n... # c\n", "a: |\n  x\n---\nexecutables:\n  a:\n",
    "executables:\n  " + "k" * 1100 + ":\n", "executables:\n  '" + "k" * 1100 + "':\n",
]

ATOMS = [
    "executables:", "  foo:", "  bar: baz", "- a", "  - b", "name: x", "# c", "", "  ", "---", "...",
    "executables: [a, b]", "executables: {a: 1}", "x: |", "  text", "\tfoo:", "  'q': z", "  \"d\":", "executables: ~",
    "  foo: # c", "  123:", "  yes:", "  : x", "executables: a", "  x: y: z", "    nested: 1", " odd:",
    "  {a: b}", "  [a, b]", "    - c", "  - d: e", "    f: g", "x: >-", "  \"s p\": 1", "  c", "executables: [a,",
    "  b]", "a: 'q'", "  - 'r'", "  'k' : v", "executables: ''", "  null:", "  1.5:", "  a.b:", "x: y # z",
]


ALPHABET = " \n:-#'\"{}[],?&*!|>%@`~0x1.e_ab\\"


def mutate(rng: random.Random, src: str) -> str:
    """1-3 random edits of a realistic pubspec: insert, delete or replace a character, or re-indent a line."""
    for _ in range(rng.randint(1, 3)):
        i = rng.randint(0, len(src))
        op = rng.randrange(4)
        if op == 0:
            src = src[:i] + rng.choice(ALPHABET) + src[i:]
        elif op == 1:
            src = src[:i] + src[i + 1:]
        elif op == 2:
            src = src[:i] + rng.choice(ALPHABET) + src[i + 1:]
        else:
            lines = src.split('\n')
            j = rng.randrange(len(lines))
            lines[j] = ' ' * rng.randint(0, 4) + lines[j].lstrip(' ')
            src = '\n'.join(lines)
    return src


def main() -> int:
    out = sys.argv[1]
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 3000
    rng = random.Random(25)
    cases = [(s, True) for s in REALISTIC] + [(s, False) for s in HAND]
    for _ in range(count):
        lines = [rng.choice(ATOMS) for _ in range(rng.randint(1, 7))]
        cases.append(('\n'.join(lines) + rng.choice(['', '\n']), False))
    for _ in range(count):
        cases.append((mutate(rng, rng.choice(REALISTIC)), False))
    seen = set()
    with open(out, 'w', encoding='UTF-8', newline='\n') as f:
        for src, accept in cases:
            if src in seen:
                continue
            seen.add(src)
            rec = {'src': src, 'want': expect(src), 'accept': accept}
            f.write(json.dumps(rec, ensure_ascii=True, sort_keys=True) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
