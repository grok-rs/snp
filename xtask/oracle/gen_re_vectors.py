"""Generate snp-pyre's CPython 3.12 `re` truth tables (docs/research/01 Addendum §A.8; WBS WP-13).

Run through `cargo xtask pyre-vectors` (never directly in CI): it needs CPython 3.12.3 (`/usr/bin/python3.12 -I`)
and writes, deterministically,

* crates/snp-pyre/tests/vectors/*.json - acceptance (ok / error class, message, position, str(e), warnings) and
  match results (re.search / re.match / re.fullmatch; spans for bytes patterns) for
  - edge.json: a hand-written edge list covering every rule and message of §A.6.2 and the rows of §A.2-§A.5;
  - corpus.json: the files/exclude/pygrep-entry patterns of pre-commit 4.7.0, pre-commit-hooks 6.0.0 and
    pygrep-hooks (plus the other reference repositories' YAML), over real repository paths;
  - fuzz_str.json / fuzz_bytes.json: grammar-fuzzed patterns (valid and mutated);
  - nesting.json: nesting rows evaluated at pre-commit's stack depth (484 groups accepted, 485 rejected);
  - haystacks.json, classes.json (\\w \\d \\s per code point, IGNORECASE literal sets), names.json (\\N{}).
* crates/snp-pyre/src/generated/names312.rs - NameAliases accepted by 3.12, CJK unified ideograph ranges and
  the code points assigned in UCD 15.0.

Match results over a shared haystack set are stored as (count, FNV-1a-64 of the matching indices) (§A.8 item 2);
per-pattern generated haystacks are stored inline with explicit results.

Reference files are untrusted data: YAML is parsed with yaml.safe_load, Python sources with ast.parse; nothing
from them is executed.
"""
from __future__ import annotations

import argparse
import ast
import json
import os
import random
import re
import re._constants as C
import re._parser as P
import signal
import subprocess
import sys
import tempfile
import unicodedata
import warnings

import yaml

MAXGROUPS_PAD = None  # calibrated stack padding for the nesting rows

FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
MASK = (1 << 64) - 1


def fnv(values):
    """FNV-1a-64 over 32-bit little-endian integers."""
    h = FNV_OFFSET
    for v in values:
        for b in int(v).to_bytes(4, 'little'):
            h ^= b
            h = (h * FNV_PRIME) & MASK
    return '%016x' % h


class Timeout(Exception):
    pass


def _alarm(signum, frame):
    raise Timeout()


signal.signal(signal.SIGALRM, _alarm)


def with_timeout(seconds, fn, *args):
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        return fn(*args)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)


# --------------------------------------------------------------------------------------------------------------
# compile outcomes
# --------------------------------------------------------------------------------------------------------------

def encode(pattern, mode):
    return pattern if mode == 's' else pattern.encode('utf-8')


def outcome(pattern, mode, flags=0, pad=0):
    """re.compile(...) as a JSON-able dict."""
    src = encode(pattern, mode)
    re.purge()

    def go():
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter('always')
            rx = re.compile(src, flags)
        ws = [str(w.message) for w in caught if issubclass(w.category, FutureWarning)]
        return rx, ws

    try:
        rx, ws = padded(pad, go)
    except re.error as e:
        return None, {'t': 're.error', 'msg': e.msg, 'pos': e.pos, 'str': str(e)}
    except OverflowError as e:
        return None, {'t': 'OverflowError', 'str': str(e)}
    except ValueError as e:
        return None, {'t': 'ValueError', 'str': str(e)}
    except RecursionError:
        return None, {'t': 'RecursionError', 'str': 'maximum recursion depth exceeded'}
    out = {'ok': True}
    if ws:
        out['w'] = ws
    return rx, out


def padded(n, fn):
    if n <= 0:
        return fn()
    return padded(n - 1, fn)


# --------------------------------------------------------------------------------------------------------------
# haystacks
# --------------------------------------------------------------------------------------------------------------

STR_ALPHABET = [
    'a', 'b', 'c', 'x', 'A', 'B', 'Z', '0', '1', '9', '_', ' ', '\n', '\r', '\t', '.', '/', '-', 'é', 'É',
    'ß', 'ẞ', 'K', 'k', 'K', 'ı', 'İ', 'i', 'I', 'ſ', 's', 'S', 'ͅ', 'ι',
    'Σ', 'σ', 'ς', '²', '①', ' ', ' ', '\x1c', '\x1f', '\U0001f600', '{', '}', '[',
    ']', '(', ')', '\\', '$', '^', '*', '+', '?', '|', '#', '٣', 'ǅ', 'ǆ', 'Ǆ', '́', '\x00',
    'क', 'ि', 'y', 'e', 'n',
]

BYTE_ALPHABET = [
    0x61, 0x62, 0x63, 0x78, 0x41, 0x42, 0x5A, 0x30, 0x31, 0x5F, 0x20, 0x0A, 0x0D, 0x09, 0x0B, 0x0C, 0x2E, 0x2F, 0x2D,
    0x23, 0x24, 0x5C, 0x7B, 0x7D, 0x5B, 0x5D, 0x28, 0x29, 0x00, 0x7F, 0x80, 0xC3, 0xA9, 0x89, 0xE2, 0x82, 0xAC, 0xFF,
    0xFE, 0x1C, 0xC9, 0xE9, 0x69, 0x49, 0x6B, 0x4B, 0x73, 0x53, 0x65, 0x6E, 0x79,
]

TRICKY_PATHS = [
    '', '\n', '\n\n', 'a', 'a\n', 'a\n\n', 'a\r', 'a\r\n', '\na', 'a\nb', 'a\nb\n', 'setup.py', 'setup.py\n',
    'src/main.rs', 'docs/index.md', 'tests/test_x.py', 'tests/x_test.py', '.pre-commit-config.yaml',
    '.pre-commit-hooks.yaml', 'a/b/c/d/e/f/g/h/i/j/k.txt', 'café.txt', 'café.txt', 'straße.py',
    'STRASSE.py', 'Kelvin.txt', 'kelvin.txt', 'KELVIN.TXT', 'İstanbul.md', 'istanbul.md', 'ı.md',
    'I.md', 'ſtraight', 'Σσς.txt', 'x².txt', '①.txt', 'nbsp file', 'line sep',
    'ctrl\x1cfile', 'emoji\U0001f600.png', '{{cookiecutter.project_name}}/setup.py', '[[x]].md', 'back\\slash',
    'dir with space/file name.txt', '-leading-dash', '#hash', 'trailing.', 'a' * 300, '/'.join(['d'] * 40) + '/f',
    'hindi/हिन्दी.txt', 'arabic/٣.txt', 'mixed/ǅx', 'tab\tfile',
    'CON', 'con.txt', 'lpt1', 'aux/x', 'file.min.js', 'vendor/lib.js', 'a.js', 'a.ts', 'a.JS', 'Makefile',
    'node_modules/x/index.js', 'migrations/0001_initial.py', 'x.py\n', 'x.py\n\n', 'x.pyc', '.git/config',
    'snapshots/__snapshots__/x.snap', 'README', 'README.md', 'readme.MD', 'a.b.c', '...', '.hidden', 'x/.hidden/y',
    'test', 'tests', 'testing', 'test_', '_test', 'a_test.go', 'x' * 3 + '\n',
]


def str_fuzz_haystacks(rnd, n):
    out = ['', '\n', '\n\n', 'a\n', 'a', 'aa', 'ab', 'ba', 'a\nb', '\na']
    seen = set(out)
    while len(out) < n:
        k = rnd.choice([0, 1, 1, 2, 2, 2, 3, 3, 4, 5, 6])
        s = ''.join(rnd.choice(STR_ALPHABET) for _ in range(k))
        if rnd.random() < 0.15:
            s += '\n'
        if s not in seen:
            seen.add(s)
            out.append(s)
    return out


def bytes_fuzz_haystacks(rnd, n):
    out = [b'', b'\n', b'\n\n', b'a\n', b'a', b'ab', b'\xff', b'\xc3\xa9', b'a\r\n', b'\x00']
    seen = set(out)
    while len(out) < n:
        k = rnd.choice([0, 1, 1, 2, 2, 3, 3, 4, 5, 6, 8])
        s = bytes(rnd.choice(BYTE_ALPHABET) for _ in range(k))
        if rnd.random() < 0.2:
            s += b'\n'
        if s not in seen:
            seen.add(s)
            out.append(s)
    return out


def repo_paths(refs, limit, rnd):
    paths = []
    for name in sorted(os.listdir(refs)):
        root = os.path.join(refs, name)
        if not os.path.isdir(root) or name.endswith('.git'):
            continue
        for d, dirs, files in os.walk(root):
            dirs[:] = sorted(x for x in dirs if x != '.git')
            for f in sorted(files):
                rel = os.path.relpath(os.path.join(d, f), root).replace(os.sep, '/')
                paths.append(rel)
    rnd.shuffle(paths)
    paths = sorted(set(paths[:limit]))
    return TRICKY_PATHS + paths + [p + '\n' for p in paths[:50]] + [p.upper() for p in paths[:50]]


def ref_lines(refs, limit, rnd):
    """Byte lines (pygrep by-line haystacks) from reference sources, plus adversarial ones."""
    lines = set()
    for name in ('pre-commit', 'pre-commit-hooks', 'pygrep-hooks'):
        root = os.path.join(refs, name)
        for d, dirs, files in os.walk(root):
            dirs[:] = sorted(x for x in dirs if x != '.git')
            for f in sorted(files):
                if not f.endswith(('.py', '.rst', '.md', '.yaml', '.cfg', '.txt')):
                    continue
                try:
                    with open(os.path.join(d, f), 'rb') as fh:
                        data = fh.read()
                except OSError:
                    continue
                for line in data.splitlines(keepends=True):
                    if 0 < len(line) <= 160:
                        lines.add(line)
    lines = sorted(lines)
    rnd.shuffle(lines)
    adversarial = [
        b'x = eval(y)\n', b'# noqa\n', b'# noqa: F401\n', b'# NOQA\n', b'# type: ignore\n', b'# type: ignore[x]\n',
        b'# type: int\n', b'logger.warn(x)\n', b'warnings.warn(x)\n', b'mck.assert_called()\n',
        b'mock.Mock.assert_called_once_with(x)\n', b'Use `code` here\n', b'``x``y\n', b'.. note::\n', b'.. note:\n',
        b'\xef\xbf\xbd\n', b'caf\xc3\xa9\n', b'\xff\xfe\n', b'tab\tline\r\n', b'/tmp/x\n', b'#/tmp/x\n', b'$/tmp/x\n',
        b'TODO\n', b'todo\n', b'\n', b'a', b'end without newline',
    ]
    return adversarial + lines[:limit]


# --------------------------------------------------------------------------------------------------------------
# sampling strings from a parsed pattern (positives)
# --------------------------------------------------------------------------------------------------------------

def sample(sp, rnd, istext, groups, budget):
    out = []
    for op, av in sp.data:
        if len(out) > budget:
            break
        if op is C.LITERAL:
            out.append(chr(av))
        elif op is C.NOT_LITERAL:
            c = rnd.choice(STR_ALPHABET if istext else [chr(b) for b in BYTE_ALPHABET])
            out.append(c if ord(c) != av else 'z')
        elif op is C.ANY:
            out.append(rnd.choice(STR_ALPHABET if istext else [chr(b) for b in BYTE_ALPHABET]))
        elif op is C.IN:
            out.append(sample_set(av, rnd, istext))
        elif op is C.BRANCH:
            out.append(sample(rnd.choice(av[1]), rnd, istext, groups, budget))
        elif op is C.SUBPATTERN:
            text = sample(av[-1], rnd, istext, groups, budget)
            if av[0]:
                groups[av[0]] = text
            out.append(text)
        elif op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
            lo, hi = av[0], av[1]
            hi = min(hi, lo + 3)
            if lo > 12:
                lo = hi = 12
            for _ in range(rnd.randint(lo, max(lo, hi))):
                out.append(sample(av[2], rnd, istext, groups, budget))
        elif op is C.GROUPREF:
            out.append(groups.get(av, ''))
        elif op is C.GROUPREF_EXISTS:
            branch = av[1] if av[0] in groups else av[2]
            if branch is not None:
                out.append(sample(branch, rnd, istext, groups, budget))
        elif op is C.ATOMIC_GROUP:
            out.append(sample(av, rnd, istext, groups, budget))
    return ''.join(out)


def sample_set(items, rnd, istext):
    members = [x for x in items if x[0] is not C.NEGATE]
    negate = any(x[0] is C.NEGATE for x in items)
    pool = STR_ALPHABET if istext else [chr(b) for b in BYTE_ALPHABET]
    if negate or not members:
        return rnd.choice(pool)
    op, av = rnd.choice(members)
    if op is C.LITERAL:
        return chr(av)
    if op is C.RANGE:
        return chr(rnd.randint(av[0], min(av[1], av[0] + 300)))
    if op is C.CATEGORY:
        cat = {
            C.CATEGORY_DIGIT: '05٣', C.CATEGORY_NOT_DIGIT: 'a-é', C.CATEGORY_SPACE: ' \t\n\x1c ',
            C.CATEGORY_NOT_SPACE: 'abé', C.CATEGORY_WORD: 'aZ_9é²', C.CATEGORY_NOT_WORD: '-. ́',
        }.get(av, 'a')
        if not istext:
            cat = ''.join(c for c in cat if ord(c) < 128) or 'a'
        return rnd.choice(cat)
    return rnd.choice(pool)


def positives(pattern, mode, flags, rnd, n=4):
    try:
        with warnings.catch_warnings():
            warnings.simplefilter('ignore')
            sp = P.parse(encode(pattern, mode), flags)
    except Exception:
        return []
    out = []
    for _ in range(n):
        try:
            s = sample(sp, rnd, mode == 's', {}, 24)[:24]
        except RecursionError:
            continue
        out.append(s)
    variants = []
    for s in out[:2]:
        variants += [s + '\n', 'x' + s, s.upper(), s.lower(), s.replace('i', 'ı') if mode == 's' else s]
    res = []
    seen = set()
    for s in out + variants:
        if mode == 'b':
            try:
                s = s.encode('latin-1')
            except UnicodeEncodeError:
                continue
        elif any(0xD800 <= ord(ch) <= 0xDFFF for ch in s):
            continue  # a Rust &str holds no surrogate (and no UTF-8 path does, 01 Addendum §A.5.4)
        if s not in seen:
            seen.add(s)
            res.append(s)
    return res[:8]


# --------------------------------------------------------------------------------------------------------------
# match tables
# --------------------------------------------------------------------------------------------------------------

def run_ops(rx, hay, mode):
    s = rx.search(hay)
    m = rx.match(hay) is not None
    f = rx.fullmatch(hay) is not None
    if mode == 's':
        return (s is not None, m, f)
    return (s.span() if s else None, m, f)


def match_row(rx, mode, haystacks, own):
    """Results over the shared set (count + FNV per op) and the own haystacks (explicit); None on a timeout
    (catastrophic backtracking in CPython)."""
    try:
        return with_timeout(5.0, _match_row, rx, mode, haystacks, own)
    except Timeout:
        return None
    except SystemError:
        # CPython 3.12.3 bug ("The span of capturing group is wrong, please report a bug for the re module"):
        # no truth to record for this pattern.
        return None


def is_utf8(h):
    try:
        h.decode('utf-8')
    except UnicodeDecodeError:
        return False
    return True


def line_shaped(h):
    """A pygrep by-line haystack: at most one b'\\n', at the end (snp's Haystack::Line precondition)."""
    return b'\n' not in h[:-1]


def _match_row(rx, mode, haystacks, own):
    """Bytes patterns: `s` holds (index, start, end) of every search hit, `sl` the same for the line-shaped
    haystacks only, and `m`/`f` cover only the UTF-8 haystacks (snp's re.match/re.fullmatch take `&str`)."""
    hits = ([], [], [])
    line_hits = []
    for i, h in enumerate(haystacks):
        s, m, f = run_ops(rx, h, mode)
        if mode == 's':
            if s:
                hits[0].append(i)
        elif s is not None:
            hits[0].extend((i, s[0], s[1]))
            if line_shaped(h):
                line_hits.extend((i, s[0], s[1]))
        textual = mode == 's' or is_utf8(h)
        if m and textual:
            hits[1].append(i)
        if f and textual:
            hits[2].append(i)
    count_s = len(hits[0]) if mode == 's' else len(hits[0]) // 3
    row = {
        's': [count_s, fnv(hits[0])],
        'm': [len(hits[1]), fnv(hits[1])],
        'f': [len(hits[2]), fnv(hits[2])],
    }
    if mode == 'b':
        row['sl'] = [len(line_hits) // 3, fnv(line_hits)]
    own_rows = []
    for h in own:
        s, m, f = run_ops(rx, h, mode)
        if mode == 's':
            own_rows.append([h, s, m, f])
        else:
            textual = is_utf8(h)
            own_rows.append([h.hex(), list(s) if s is not None else None, m if textual else None,
                             f if textual else None])
    if own_rows:
        row['own'] = own_rows
    return row


FLAG_BITS = {'i': re.IGNORECASE, 'ms': re.MULTILINE | re.DOTALL, 'ims': re.IGNORECASE | re.MULTILINE | re.DOTALL}


def conditional_reentry(pattern, mode, flags):
    """A conditional `(?(g)...)` inside its own group g, under a repetition: on a re-entry of g, CPython tests
    "previous end >= current start" (`SRE_OP_GROUPREF_EXISTS` on the marks), which no fancy-regex construct can
    express. Marked so the test can tell."""
    try:
        with warnings.catch_warnings():
            warnings.simplefilter('ignore')
            sp = P.parse(encode(pattern, mode), flags)
    except Exception:
        return False
    stack = [(sp, (), False)]
    while stack:
        cur, open_groups, repeated = stack.pop()
        for op, av in cur.data:
            if op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
                stack.append((av[2], open_groups, repeated or av[1] > 1))
            elif op is C.BRANCH:
                stack.extend((b, open_groups, repeated) for b in av[1])
            elif op is C.SUBPATTERN:
                inner = open_groups + (av[0],) if av[0] else open_groups
                stack.append((av[-1], inner, repeated))
            elif op in (C.ASSERT, C.ASSERT_NOT):
                stack.append((av[1], open_groups, repeated))
            elif op is C.ATOMIC_GROUP:
                stack.append((av, open_groups, repeated))
            elif op is C.GROUPREF_EXISTS:
                if av[0] in open_groups and repeated:
                    return True
                stack.append((av[1], open_groups, repeated))
                if av[2] is not None:
                    stack.append((av[2], open_groups, repeated))
    return False


def nullable_loop(pattern, mode, flags):
    """The pattern repeats a body that can match the empty string in a way snp cannot run like CPython (D8).

    CPython ends such a loop after an empty iteration *with that iteration's priority*. snp reproduces that for
    unbounded greedy loops (fancy-regex's epsilon check) and lazy ones, but not for bounded or possessive loops, nor
    for `{2,}` loops whose captures a back-reference or conditional can observe: there the span end of a
    leftmost-first match may differ (never whether there is a match). Marked so the test can tell."""
    try:
        with warnings.catch_warnings():
            warnings.simplefilter('ignore')
            sp = P.parse(encode(pattern, mode), flags)
    except Exception:
        return False
    observable = bool(re.search(r'\\[1-9]|\(\?P=|\(\?\(', pattern))

    def has_group(sub):
        todo = [sub]
        while todo:
            for op, av in todo.pop().data:
                if op is C.SUBPATTERN and av[0]:
                    return True
                if op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
                    todo.append(av[2])
                elif op is C.SUBPATTERN or op is C.ATOMIC_GROUP:
                    todo.append(av[-1] if op is C.SUBPATTERN else av)
                elif op is C.BRANCH:
                    todo.extend(av[1])
                elif op in (C.ASSERT, C.ASSERT_NOT):
                    todo.append(av[1])
        return False

    stack = [sp]
    while stack:
        cur = stack.pop()
        for op, av in cur.data:
            if op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
                if av[2].getwidth()[0] == 0 and av[1] > 1:
                    unbounded = av[1] == C.MAXREPEAT
                    exact = unbounded and (op is C.MIN_REPEAT or (
                        op is C.MAX_REPEAT and (av[0] <= 1 or not (observable and has_group(av[2])))))
                    if not exact:
                        return True
                stack.append(av[2])
            elif op is C.BRANCH:
                stack.extend(av[1])
            elif op is C.SUBPATTERN:
                stack.append(av[-1])
            elif op in (C.ASSERT, C.ASSERT_NOT):
                stack.append(av[1])
            elif op is C.ATOMIC_GROUP:
                stack.append(av)
            elif op is C.GROUPREF_EXISTS:
                stack.append(av[1])
                if av[2] is not None:
                    stack.append(av[2])
    return False


def vector(pattern, mode, haystacks, set_name, rnd, flags='', pad=0, own=True):
    fl = FLAG_BITS.get(flags, 0)
    rx, out = outcome(pattern, mode, fl, pad)
    row = {'p': pattern, 'md': mode}
    if flags:
        row['fl'] = flags
    row.update(out)
    if rx is not None and haystacks is not None:
        own_h = positives(pattern, mode, fl, rnd) if own else []
        mr = match_row(rx, mode, haystacks, own_h)
        if mr is not None:
            row['set'] = set_name
            row.update(mr)
            if mode == 'b' and nullable_loop(pattern, mode, fl):
                row['nl'] = True
            if conditional_reentry(pattern, mode, fl):
                row['cr'] = True
    return row


# --------------------------------------------------------------------------------------------------------------
# the edge list (every rule and message of §A.6.2, every row of §A.2-§A.5)
# --------------------------------------------------------------------------------------------------------------

EDGE_LIST_BOTH = ['', '|', 'a|', '|a', '(|a)', '(a|)', 'a||b', '()', '(?:)', 'a|b|', '(|)', '((|))']

EDGE_BOTH = r'''
\
a\
(?#a\
[a\
a{1\
(?P<a\
(?x)a#b\
\a\b\f\n\r\t\v\\
\A\Z\b\B\d\D\s\S\w\W
\z
\x41
\x4
\x
\xg1
\x{41}
\0
\07
\077
\0777
\1
(a)\1
(a)\2
(a)\10
(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\10
(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\11
\123
\377
\400
\477
\8
\9
\18
(a\1)
(?P<a>x)(?P=a)
(?P<a>x(?P=a))
(?P=a)
(?P<a>x)(?P<a>y)
(?P<1a>x)
(?P<>x)
(?P<a
(?P<a>
(?P
(?Px)
(?P>x)
(?P=)
(?P=a
(?P=1)
(?
(?<a>x)
(?<
(?<x
(?<=a)b
(?<!a)b
a(?=b)
a(?!b)
(?<=a|bc)
(?<=a*)
(?<=a{1,2})
(?<=a{2})b
(?<=(a)\1)
(a)(?<=\1)
(?<=(?P<x>a)(?P=x))
(?<=a(?<=b))c
(?<=\b)a
(?<=$)a
(?<=a{4294967294})
(?<=(?:a{65536}){65536})
(?(1)a|b)
(a)(?(1)b|c|d)
(?(0)a)
(?(-1)a)
(?(+1)a)(b)
(a)?(?(1)b|c)
(a)?(?(1)b)
(?(1)a)(b)
(?(2)a)(b)
(?(a)b)
(?P<a>x)(?(a)b|c)
(?(1a)b)
(?(00)a)
(?(01)a)(b)
(?(1073741823)a)
(?(99999999999999999999)a)
(?(1)a|b|c)
(?(1)a
(?(1
(?()a)
(?>a)
(?>a|ab)c
a++
a*+
a?+
a{1,2}+
a{,3}
a{,}
a{}
a{
a{1,2
a{x}
a{-1}
a{ 2}
x{2,1}
x{2}{3}
x*{2}
x**
x*?*
x+?+
x?+*
{1}
{,}
{
{x}
{{cookiecutter.project_name}}
\.{js,ts}$
a{4294967294}
a{4294967295}
a{4294967296}
a{0,4294967295}
a{1,4294967295}
a{,4294967295}
a{99999999999999999999}
a{99999999999,}
a{00000000000000000000005}
a{1000}
^*
$+
\b*
\A?
\Z{2}
(?=a)*
(?!a)+
(?<=a)?
(?:)*
(?:)+
()*
(?:^)*
(?:$)?
(?:\b|\B)*
(?:(?<!a){2})+
(?:a|(?=b))*
(?:\b)*?
(?:(?=a))*+
(?:a{0})*
a{0}
(a){0}\1
(?=(a)){0}
a(?i)
(a)(?i)
(?i)|(?m)
(?:(?i)a)
(?i)(?m)a
(?#c)(?i)a
(?i)a(?m)
(?x)  (?i)a
(?i-i:a)
(?i-:a)
(?-i)a
(?-:a)
(?i
(?i:
(?i-
(?i-s
(?i)a
(?i:a)b
(?-i:a)
(?i-s:a)
(?s).
(?m)^a$
(?a)
(?a:x)
(?a)\w
(?ai)k
(?imsxa)a
(?au)a
(?u)(?a)x
(?a)(?u)x
(?a)(?u:x)
(?u)(?a:x)
(?-u:x)
(?-a:x)
(?L)a
(?L:a)
(?aL)a
(?a)(?L)x
(?t)a
(?t)a*
(?t)a*?
(?t)a*+
(?t)(?<=a|bc)a*
(?t)a*(?<=a|bc)
(?t:a)
(?-t:a)
(?i-i)
(?ii)a
(?q)
(?i)(?q)
(?iq:a)
(?i\x)
(?\x)
(?é)
(?x)( ?:a)
(?x)(? :a)
(?x)a{ 2}
(?x)a {2}
(?x)\ a
(?x)\#a
(?x)a#comment
(?x)a#comment\nb
(?x)[ ]
(?x)[#]
(?x)[a b]
(?x)a b|c d
(?x)a|(?-x: b)
(?-x:a b)(?x)
(?x: a )b c
(?x)(a  # c
(?x)
(?x)
[[]
[[:alpha:]]
[[a]
[^[]
[a--b]
[a-z--b]
[--a]
[a--]
[a&&b]
[a~~b]
[a||b]
[&&]
[a-&&]
[---]
[\b]
[\B]
[\A]
[\z]
[\Z]
[\e]
[\d]
[\D-z]
[a-\d]
[\w-.]
[\x41-\w]
[a-]
[-a]
[]a]
[^]a]
[]
[^]
[
[^
[a
[a-
[a-b
[z-a]
[\1]
[\0]
[\7]
[\8]
[\9]
[\177]
[\477]
[\x4]
[a\]b]
[\\]
[^\\]
[\n-\r]
[.]
[*+?]
[(){}]
[$^]
[a-a]
[aa]
[^a]
[a]
[\^]
(?i)[a-z]
(?i)[^a]
(?i)[k]
(?i)k
(?i)K
(?i)s
(?i)\xdf
(?i)i
(?i)I
(?a)\s
(?a)[\s]
(?a)\d
(?a)\b
(?a)\B
(?i)(a)\1
(?i)(k)\1
(?i)(s)\1
a\s
a\S
\w+
\W+
.
.*
(?s).
(?s:.)a.
^
$
^$
\A$
^\Z
\A\Z
a$
a$\n
a\Z
^a
\Aa
a$b
(?m)a$
(?m)^a
(?m)^$
(?m:^)a
\ba
a\b
\Ba
a\B
\b
\B
a|b|c
(a|b|c)
(?:a|b)c
a()b
a(?:)b
(?:a)
((a))
(?:(?:(?:a)))
a{2}b{3}
a{2,}
a{,2}
a{2,3}?
a*?b
a+?b
a??b
\.
\/
\:
\-
\@
\%
\
\#
\~
\é
\😀
A
\U00000041
\N{EM DASH}
é
😀
a\nb
a
b
))
a)
(a))
a)b(
'''

EDGE_STR_ONLY = r'''
A
\u004
\u
\U00000041
\U0000004
\U00110000
\U0010FFFF
\uD800
\ud800
[\ud800-\udfff]
[^\ud800]
\udfff+
[\ud800\uD801]
(?i)\ud800
\N{EM DASH}
\N{em dash}
\N{EM DASH
\N{}
\N
\Nx
\N{NOPE}
\N{LATIN SMALL LETTER GHA}
\N{LATIN SMALL LETTER A}
\N{latin small letter a}
\N{LATIN_SMALL_LETTER_A}
\N{ LATIN SMALL LETTER A}
\N{LATIN SMALL LETTER A }
\N{LATIN  SMALL LETTER A}
\N{LATIN SMALL LETTER-A}
\N{CJK UNIFIED IDEOGRAPH-4E00}
\N{CJK UNIFIED IDEOGRAPH-4e00}
\N{cjk unified ideograph-4E00}
\N{CJK UNIFIED IDEOGRAPH-2EBF0}
\N{CJK UNIFIED IDEOGRAPH-9FFF}
\N{CJK UNIFIED IDEOGRAPH-A000}
\N{CJK COMPATIBILITY IDEOGRAPH-F900}
\N{cjk compatibility ideograph-f900}
\N{HANGUL SYLLABLE GA}
\N{hangul syllable GA}
\N{HANGUL SYLLABLE ga}
\N{HANGUL SYLLABLE GAG}
\N{HANGUL SYLLABLE}
\N{TANGUT IDEOGRAPH-17000}
\N{NUSHU CHARACTER-1B170}
\N{KHITAN SMALL SCRIPT CHARACTER-18B00}
\N{LF}
\N{NULL}
\N{BOM}
\N{LATIN CAPITAL LETTER A WITH MACRON AND GRAVE}
\N{GARAY CAPITAL LETTER A}
[\N{EM DASH}]
[\N{EM DASH}-\N{HORIZONTAL BAR}]
[\N{NOPE}]
(?P<é>x)(?P=é)
(?P<ǅ>x)
(?P<²>x)
(?P<_1>x)
(?P<a-b>x)
(?P<a b>x)
(?P<١>x)
(?(١)a)
(?(٣)a)
(?P<a>x)(?(a)y)
(?L)a
(?u)a
(?au)
(?a)é
(?ai)é
(?i)é
(?i)ǅ
(?i)ß
(?i)ẞ
(?i)K
(?i)ſ
(?i)ς
(?i)σ
(?i)ͅ
(?i)ι
(?i)µ
(?i)ı
(?i)İ
(?i)[ı]
(?i)[İ]
(?i)[h-j]
(?i)[^İ]
(?i)[\w]
(?i)\w
(?i)[^\W]
\w
\d
\s
\W
[\s\S]
[\w\W]
²
[²]
(?i)straße

'''

EDGE_BYTES_ONLY = r'''
(?u)x
(?a)(?L)x
(?L)(?a)x
(?aL)x
(?L)\w
(?L)(?i)a
(?P<é>x)
(?P<\xe9>x)
\é
[é]
[\x80-\xff]
\xff
(?i)\xc9
(?i)é
[^\x00-\x7f]
A
\U00000041
\N{EM DASH}
é+
\xc3\xa9+
(?i)[a-z]
(?i)[\xc0-\xde]
'''


def edge_patterns():
    both = EDGE_LIST_BOTH + EDGE_BOTH.split('\n')[1:-1]
    str_only = EDGE_STR_ONLY.split('\n')[1:-1]
    bytes_only = EDGE_BYTES_ONLY.split('\n')[1:-1]
    rows = []
    for p in both:
        p = p.replace('\\n', '\n') if p.startswith('(?x)a#comment\\n') else p
        rows.append((p, 's'))
        rows.append((p, 'b'))
    rows += [(p, 's') for p in str_only]
    rows += [(p, 'b') for p in bytes_only]
    # patterns with newlines (line/column suffix of str(e))
    extras = [
        '(?x)\n(a\n  (b', '(?x)\n  [a', 'a\nb(', '\n\\', '(?x)a\n  #c\n  b{2,1}', 'ab\n(?i)', '(\n\n\n',
        '(?x)(\n?P<a>x)', 'x\n**', '\n\n[z-a]',
    ]
    for p in extras:
        rows += [(p, 's'), (p, 'b')]
    # a long verbose pattern, a large alternation, wide classes
    rows += [('(?x)' + ' | '.join('k%d' % i for i in range(300)), 's')]
    rows += [('|'.join('w%d' % i for i in range(2000)), 's')]
    rows += [('[\\x00-\\U0010ffff]', 's'), ('[^\\x00-\\U0010ffff]', 's'), ('(?i)[\\x00-\\U0010ffff]', 's')]
    rows += [('[\\x00-\\xff]', 'b'), ('[^\\x00-\\xff]', 'b')]
    # review round 1 (appended so that the rows above keep their haystack draws): `\U` past a C int (D7),
    # nullable loop bodies (`MAX_UNTIL`'s empty-iteration exit), lazy bodies fancy-regex's optimizer rewrites,
    # and the search-only charset prefix of a scoped `(?a:…)` set.
    for p in ('\\U80000000', '\\UFFFFFFFF', '[\\U80000000-\\U90000000]', '(?:x*|y)+', '(?:$|.)*', '(?:x*|y)+z',
              '(?:x*|y){2,}', '(?:a|\\b)*', '(?=x)(x+?)*', '(?<=a)(x+?)*', '(?=x)((x)+?)*', '(?=a)a*b??a*'):
        rows += [(p, 's'), (p, 'b')]
    rows += [(p, 's') for p in ('(?a:\\W)', '(?a:[\\SA-k])', '(?ims)(?:(?a:[\\S]a))', '(?a:\\Wa|\\Wb)', '(?a:\\W|x)',
                                '(?i)(?a:[\\Sa])', '((?a:[^\\w]))', '(?a:\\W)*x')]
    seen = set()
    out = []
    for r in rows:
        if r not in seen:
            seen.add(r)
            out.append(r)
    return out


# --------------------------------------------------------------------------------------------------------------
# the grammar fuzzer
# --------------------------------------------------------------------------------------------------------------

FUZZ_LITERALS_STR = [
    'a', 'b', 'c', 'x', 'A', 'B', 'K', 'k', 's', 'S', 'i', 'I', '0', '1', '_', ' ', '/', '-', '#', 'é', 'É',
    'ß', 'ı', 'İ', 'ſ', 'K', 'σ', 'ς', 'Σ', 'ͅ', '²', '①',
    '\U0001f600', 'ǅ', '٣', ' ', ']', '}', ',', ':', '<', '>', '=', '!', '&', '~', '@', '%', "'", '"',
]
FUZZ_ESCAPES = [
    r'\.', r'\*', r'\+', r'\?', r'\(', r'\)', r'\[', r'\]', r'\{', r'\}', r'\|', r'\^', r'\$', r'\\', r'\/', r'\-',
    r'\ ', r'\#', r'\n', r'\t', r'\r', r'\a', r'\f', r'\v', r'\x41', r'\x7f', r'\xe9', r'\0', r'\012', r'\101',
    r'\d', r'\D', r'\s', r'\S', r'\w', r'\W', r'\b', r'\B', r'\A', r'\Z',
]
FUZZ_ESCAPES_STR = [r'é', r'İ', r'\U0001F600', r'\N{EM DASH}', r'\N{LATIN SMALL LETTER SHARP S}', r'\uD800']
FUZZ_CLASS_ITEMS = [
    'a', 'b', 'z', 'A', 'Z', '0', '9', '_', '-', '.', ' ', '/', '^', '[', '&', '~', '|', r'\]', r'\\', r'\d', r'\w',
    r'\s', r'\D', r'\W', r'\S', r'\b', r'\n', r'\x41', r'\x80', r'\0', r'\177', 'a-z', 'A-Z', '0-9', 'a-c', 'x-z',
]
FUZZ_CLASS_ITEMS_STR = ['é', 'ı', 'ſ', 'K', 'σ', 'ͅ', '²', r'Ā-ſ',
                        r' -⁯', '\U0001f600', 'k', 's', 'i', 'I']
FUZZ_FLAGS = ['i', 'm', 's', 'x', 'a', 'u', 'L', 'is', 'im', 'ms', 'ix', 'ai', 'ia', 'au', 'iu', 'ims']


class Fuzzer:
    def __init__(self, rnd, istext):
        self.rnd = rnd
        self.istext = istext
        self.groups = 0
        self.names = []

    def pattern(self):
        self.groups = 0
        self.names = []
        r = self.rnd
        head = ''
        if r.random() < 0.18:
            fl = r.choice(FUZZ_FLAGS)
            head = '(?%s)' % fl
        body = self.alt(0)
        p = head + body
        if r.random() < 0.08 and '(?x' not in p:
            p = '(?x)' + self.spacey(p)
        return p

    def spacey(self, p):
        out = []
        for ch in p:
            out.append(ch)
            if self.rnd.random() < 0.15:
                out.append(self.rnd.choice([' ', '  ', '\n', ' # c\n']))
        return ''.join(out)

    def alt(self, depth):
        r = self.rnd
        n = r.choice([1, 1, 1, 1, 2, 2, 3])
        return '|'.join(self.seq(depth) for _ in range(n))

    def seq(self, depth):
        r = self.rnd
        n = r.choice([0, 1, 1, 2, 2, 3, 3, 4, 5])
        return ''.join(self.piece(depth) for _ in range(n))

    def piece(self, depth):
        r = self.rnd
        atom = self.atom(depth)
        if r.random() < 0.3:
            atom += self.quant()
        return atom

    def quant(self):
        r = self.rnd
        q = r.choice(['*', '+', '?', '{2}', '{1,3}', '{,2}', '{2,}', '{0}', '{0,1}', '{3,3}', '{,}', '{}', '{x}',
                      '{1,', '{2,1}'])
        x = r.random()
        if x < 0.2:
            q += '?'
        elif x < 0.3:
            q += '+'
        return q

    def fixed(self, depth):
        """A fixed-width piece for look-behinds."""
        r = self.rnd
        k = r.choice([0, 1, 1, 2, 3])
        out = []
        for _ in range(k):
            c = r.random()
            if c < 0.5:
                out.append(self.literal())
            elif c < 0.7:
                out.append(self.cls())
            elif c < 0.8:
                out.append('.')
            elif c < 0.9:
                out.append('\\b')
            else:
                out.append(self.literal() + '{2}')
        if r.random() < 0.2:
            out = ['(?:' + ''.join(out) + '|' + ''.join(out) + ')']
        return ''.join(out)

    def literal(self):
        r = self.rnd
        if self.istext:
            return r.choice(FUZZ_LITERALS_STR)
        return r.choice([c for c in FUZZ_LITERALS_STR if ord(c) < 0x800])

    def cls(self):
        r = self.rnd
        items = FUZZ_CLASS_ITEMS + (FUZZ_CLASS_ITEMS_STR if self.istext else ['é', r'\xff'])
        n = r.choice([1, 1, 2, 2, 3, 4])
        body = ''.join(r.choice(items) for _ in range(n))
        neg = '^' if r.random() < 0.25 else ''
        return '[' + neg + body + ']'

    def atom(self, depth):
        r = self.rnd
        x = r.random()
        if depth > 4:
            x = x * 0.55
        if x < 0.28:
            return self.literal()
        if x < 0.36:
            esc = FUZZ_ESCAPES + (FUZZ_ESCAPES_STR if self.istext else [])
            return r.choice(esc)
        if x < 0.44:
            return self.cls()
        if x < 0.48:
            return '.'
        if x < 0.53:
            return r.choice(['^', '$', '\\A', '\\Z', '\\b', '\\B'])
        if x < 0.55:
            if self.groups:
                if self.names and r.random() < 0.3:
                    return '(?P=%s)' % r.choice(self.names)
                return '\\%d' % r.randint(1, self.groups)
            return '\\1'
        if x < 0.62:
            self.groups += 1
            if r.random() < 0.3:
                name = 'n%d' % self.groups
                self.names.append(name)
                return '(?P<%s>%s)' % (name, self.alt(depth + 1))
            return '(%s)' % self.alt(depth + 1)
        if x < 0.68:
            return '(?:%s)' % self.alt(depth + 1)
        if x < 0.73:
            return r.choice(['(?=', '(?!']) + self.alt(depth + 1) + ')'
        if x < 0.77:
            return r.choice(['(?<=', '(?<!']) + self.fixed(depth + 1) + ')'
        if x < 0.79:
            return '(?>' + self.alt(depth + 1) + ')'
        if x < 0.82:
            g = r.randint(1, max(1, self.groups + 1))
            no = '|' + self.seq(depth + 1) if r.random() < 0.6 else ''
            return '(?(%d)%s%s)' % (g, self.seq(depth + 1), no)
        if x < 0.88:
            on = r.choice(['i', 's', 'm', 'x', 'a', 'is', ''])
            off = r.choice(['', '', '-i', '-s', '-m', '-x', '-im'])
            if not on and not off:
                on = 'i'
            return '(?%s%s:%s)' % (on, off, self.alt(depth + 1))
        if x < 0.89:
            return '(?#%s)' % r.choice(['c', 'a b', '(', '\\)'])
        return self.literal()

    def mutate(self, p):
        r = self.rnd
        chars = list(p)
        for _ in range(r.choice([1, 1, 2, 3])):
            op = r.random()
            pos = r.randint(0, len(chars))
            if op < 0.4 and chars:
                del chars[min(pos, len(chars) - 1)]
            elif op < 0.8:
                chars.insert(pos, r.choice(list('()[]{}*+?|\\^$-:<>=!#,P') + ['\\', '(?', '{1', ',', 'x']))
            elif chars:
                i = min(pos, len(chars) - 1)
                chars[i] = r.choice(list('()[]{}*+?|\\'))
        return ''.join(chars)


def fuzz_patterns(rnd, istext, n):
    f = Fuzzer(rnd, istext)
    seen = set()
    out = []
    while len(out) < n:
        p = f.pattern()
        if rnd.random() < 0.3:
            p = f.mutate(p)
        if len(p) > 80 or p in seen:
            continue
        if not istext:
            try:
                p.encode('utf-8')
            except UnicodeEncodeError:
                continue
        seen.add(p)
        out.append(p)
    return out


# --------------------------------------------------------------------------------------------------------------
# corpus
# --------------------------------------------------------------------------------------------------------------

def corpus(refs):
    str_pats, bytes_pats = set(), set()

    def from_hook(h):
        if not isinstance(h, dict):
            return
        for k in ('files', 'exclude'):
            v = h.get(k)
            if isinstance(v, str):
                str_pats.add(v)
        if h.get('language') == 'pygrep' and isinstance(h.get('entry'), str):
            bytes_pats.add(h['entry'])

    for name in sorted(os.listdir(refs)):
        root = os.path.join(refs, name)
        if not os.path.isdir(root) or name.endswith('.git'):
            continue
        for d, dirs, files in os.walk(root):
            dirs[:] = sorted(x for x in dirs if x not in ('.git', 'node_modules', 'target'))
            for f in sorted(files):
                path = os.path.join(d, f)
                if f.endswith(('.yaml', '.yml')):
                    try:
                        with open(path, encoding='utf-8') as fh:
                            doc = yaml.safe_load(fh)
                    except Exception:
                        continue
                    if isinstance(doc, list):
                        for h in doc:
                            from_hook(h)
                    elif isinstance(doc, dict):
                        for k in ('files', 'exclude'):
                            if isinstance(doc.get(k), str):
                                str_pats.add(doc[k])
                        for repo in doc.get('repos') or []:
                            if isinstance(repo, dict):
                                for h in repo.get('hooks') or []:
                                    from_hook(h)
                elif f.endswith('.py') and name in ('pre-commit', 'pre-commit-hooks'):
                    try:
                        with open(path, encoding='utf-8') as fh:
                            tree = ast.parse(fh.read())
                    except Exception:
                        continue
                    for node in ast.walk(tree):
                        if isinstance(node, ast.Dict):
                            for k, v in zip(node.keys, node.values):
                                if (isinstance(k, ast.Constant) and k.value in ('files', 'exclude')
                                        and isinstance(v, ast.Constant) and isinstance(v.value, str)):
                                    str_pats.add(v.value)
                        if isinstance(node, ast.Call):
                            fn = node.func
                            if (isinstance(fn, ast.Attribute) and fn.attr in ('compile', 'search', 'match', 'fullmatch')
                                    and isinstance(fn.value, ast.Name) and fn.value.id == 're' and node.args
                                    and isinstance(node.args[0], ast.Constant)):
                                v = node.args[0].value
                                if isinstance(v, str):
                                    str_pats.add(v)
                                elif isinstance(v, bytes):
                                    try:
                                        bytes_pats.add(v.decode('utf-8'))
                                    except UnicodeDecodeError:
                                        pass
    return sorted(str_pats), sorted(bytes_pats)


# --------------------------------------------------------------------------------------------------------------
# Unicode tables
# --------------------------------------------------------------------------------------------------------------

def ranges_of(pred):
    out = []
    start = None
    for c in range(0x110000):
        if pred(c):
            if start is None:
                start = c
        elif start is not None:
            out.append([start, c - 1])
            start = None
    if start is not None:
        out.append([start, 0x10FFFF])
    return out


def class_tables():
    import _sre
    w = re.compile(r'\w')
    d = re.compile(r'\d')
    s = re.compile(r'\s')
    tables = {
        'word': ranges_of(lambda c: w.fullmatch(chr(c)) is not None),
        'digit': ranges_of(lambda c: d.fullmatch(chr(c)) is not None),
        'space': ranges_of(lambda c: s.fullmatch(chr(c)) is not None),
        'assigned': ranges_of(lambda c: unicodedata.category(chr(c)) != 'Cn'),
    }
    # IGNORECASE literal sets: candidates by unicode_tolower groups, verified with re.fullmatch.
    groups = {}
    for c in range(0x110000):
        groups.setdefault(_sre.unicode_tolower(c), []).append(c)
    from re._casefix import _EXTRA_CASES
    fold = {}
    for c in range(0x110000):
        if not _sre.unicode_iscased(c):
            continue
        lo = _sre.unicode_tolower(c)
        keys = [lo] + list(_EXTRA_CASES.get(lo, ()))
        cand = sorted({x for k in keys for x in groups.get(k, [])} | {c})
        rx = re.compile('(?i)' + re.escape(chr(c)))
        members = [x for x in cand if rx.fullmatch(chr(x))]
        if members != [c]:
            fold['%x' % c] = members
    tables['fold'] = fold
    return tables


def names_tables(aliases_path):
    aliases = []
    with open(aliases_path, encoding='utf-8') as fh:
        for line in fh:
            line = line.split('#', 1)[0].strip()
            if not line:
                continue
            cp, alias, _kind = line.split(';')
            cp = int(cp, 16)
            try:
                got = unicodedata.lookup(alias)
            except KeyError:
                continue
            if len(got) == 1 and ord(got) == cp:
                aliases.append((alias.upper(), cp))
    aliases = sorted(set(aliases))
    cjk = []
    for c in range(0x110000):
        try:
            ok = ord(unicodedata.lookup('CJK UNIFIED IDEOGRAPH-%04X' % c)) == c
        except KeyError:
            ok = False
        if ok:
            if cjk and cjk[-1][1] == c - 1:
                cjk[-1][1] = c
            else:
                cjk.append([c, c])
    assigned = ranges_of(lambda c: unicodedata.category(chr(c)) != 'Cn')
    return aliases, cjk, assigned


def rs_hex(c):
    """A Rust hex literal clippy's `unreadable_literal` accepts (`0x10_FFFD` style grouping past 5 digits)."""
    return '0x%04X' % c if c <= 0xFFFFF else '0x%X_%04X' % (c >> 16, c & 0xFFFF)


def write_names_rs(path, aliases, cjk, assigned):
    with open(path, 'w', encoding='utf-8', newline='\n') as fh:
        fh.write('//! Generated by `cargo xtask pyre-vectors` (`xtask/oracle/gen_re_vectors.py`) from CPython 3.12.3\n')
        fh.write('//! `unicodedata` (UCD 15.0) and the NameAliases.txt shipped with `unicode_names2` 4.0.0. Do not edit.\n')
        fh.write('//!\n//! Owner: WP-13.\n\n')
        fh.write('/// `NameAliases.txt` aliases that CPython 3.12 `unicodedata.lookup` accepts, uppercased and sorted.\n')
        fh.write('pub(crate) static ALIASES: &[(&str, u32)] = &[\n')
        for a, c in aliases:
            fh.write('    (%s, %s),\n' % (json.dumps(a), rs_hex(c)))
        fh.write('];\n\n')
        fh.write('/// Code points named `CJK UNIFIED IDEOGRAPH-XXXX` by CPython 3.12 (`is_unified_ideograph`).\n')
        fh.write('pub(crate) static CJK_UNIFIED: &[(u32, u32)] = &[\n')
        for a, b in cjk:
            fh.write('    (%s, %s),\n' % (rs_hex(a), rs_hex(b)))
        fh.write('];\n\n')
        fh.write('/// Code points assigned in UCD 15.0 (general category other than Cn).\n')
        fh.write('pub(crate) static ASSIGNED: &[(u32, u32)] = &[\n')
        for a, b in assigned:
            fh.write('    (%s, %s),\n' % (rs_hex(a), rs_hex(b)))
        fh.write('];\n')


def names_vectors(rnd, aliases):
    rows = []
    named = []
    for c in range(0x110000):
        try:
            n = unicodedata.name(chr(c))
        except ValueError:
            continue
        if n.startswith(('CJK UNIFIED IDEOGRAPH-', 'HANGUL SYLLABLE ')):
            continue
        named.append((c, n))
    values = []
    for c, n in named:
        values.append(c)
        values.append(len(n))
        values.extend(ord(ch) for ch in n)
    digest = fnv(values)
    sample = rnd.sample(named, 3000)
    cand = []
    for c, n in sample:
        cand += [n, n.lower(), n.title(), n.replace(' ', '_'), ' ' + n, n.replace('-', ' ')]
    cand += [a for a, _ in aliases] + [a.lower() for a, _ in aliases]
    for c in rnd.sample(range(0xAC00, 0xD7A4), 200):
        n = unicodedata.name(chr(c))
        cand += [n, n.lower(), 'hangul syllable ' + n[16:], n[:16] + n[16:].lower()]
    for c in [0x3400, 0x4DBF, 0x4E00, 0x9FFF, 0xA000, 0x20000, 0x2A6DF, 0x2A700, 0x2EBE0, 0x2EBF0, 0x30000, 0x323AF,
              0x323B0, 0x31350]:
        h = '%04X' % c
        cand += ['CJK UNIFIED IDEOGRAPH-' + h, 'CJK UNIFIED IDEOGRAPH-' + h.lower(), 'cjk unified ideograph-' + h,
                 'CJK UNIFIED IDEOGRAPH-0' + h]
    cand += ['TANGUT IDEOGRAPH-17000', 'TANGUT COMPONENT-001', 'NUSHU CHARACTER-1B170', 'KHITAN SMALL SCRIPT CHARACTER-18B00',
             'LATIN CAPITAL LETTER A WITH MACRON AND GRAVE', 'KEYCAP NUMBER SIGN', 'GARAY CAPITAL LETTER A', '', ' ',
             'LATIN SMALL LETTER Aé', 'x' * 300]
    seen = set()
    for name in cand:
        if name in seen:
            continue
        seen.add(name)
        try:
            got = unicodedata.lookup(name)
            cp = ord(got) if len(got) == 1 else None
        except (KeyError, TypeError):
            cp = None
        rows.append([name, cp])
    return {'digest': digest, 'count': len(named), 'lookup': rows}


# --------------------------------------------------------------------------------------------------------------
# nesting (pre-commit's stack depth)
# --------------------------------------------------------------------------------------------------------------

def calibrate_pad():
    """The stack padding at which 484 nested groups compile and 485 raise RecursionError (01 Addendum §A.4)."""
    for pad in range(0, 200):
        ok484 = outcome('(' * 484 + 'a' + ')' * 484, 's', 0, pad)[1].get('ok')
        ok485 = outcome('(' * 485 + 'a' + ')' * 485, 's', 0, pad)[1].get('ok')
        if ok484 and not ok485:
            return pad
    raise SystemExit('could not calibrate the nesting padding')


# bytes patterns are compiled only by pygrep: `python -m pre_commit.languages.pygrep` runs `main()`, which calls
# `re.compile(args.pattern.encode(), flags)` itself, from a much shallower stack than config validation (494
# nested groups compile there, 495 raise RecursionError, measured with pygrep's `main` on CPython 3.12.3). The
# probe reproduces that stack: run with `-m`, `raise SystemExit(main())`, `re.compile` called from `main`.
PYGREP_PROBE = """import json, re, sys


def main():
    out = []
    for p in json.load(sys.stdin):
        try:
            rx = re.compile(p.encode(), 0)
        except RecursionError:
            out.append(None)
            continue
        out.append([rx.search(x) is not None for x in (b'a', b'ab', b'b', b'')])
    json.dump(out, sys.stdout)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
"""


def pygrep_outcomes(patterns):
    """[hits or None (RecursionError)] of each bytes pattern compiled at pygrep's stack depth."""
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, 'snp_pygrep_probe.py'), 'w', encoding='utf-8') as fh:
            fh.write(PYGREP_PROBE)
        # `-E -s` instead of `-I`: `-m` must find the probe in its own (fresh, empty) directory.
        res = subprocess.run([sys.executable, '-E', '-s', '-m', 'snp_pygrep_probe'], cwd=tmp, check=True,
                             input=json.dumps(patterns).encode(), capture_output=True)
    return json.loads(res.stdout)


def nesting_rows(pad):
    shapes = {
        'cap': lambda n: '(' * n + 'a' + ')' * n,
        'noncap': lambda n: '(?:' * n + 'a' + ')' * n,
        'look': lambda n: '(?=' * n + 'a' + ')' * n,
        'behind': lambda n: '(?<=' * n + 'a' + ')' * n,
        'atomic': lambda n: '(?>' * n + 'a' + ')' * n,
        'flags': lambda n: '(?i:' * n + 'a' + ')' * n,
        'capq': lambda n: '(' * n + 'a' + ')*' * n,
        'branch': lambda n: '(a|' * n + 'b' + ')' * n,
        'cond': lambda n: '(a)' + '(?(1)' * n + 'a' + ')' * n,
    }
    rows = []
    for name, f in shapes.items():
        ns = [1, 2, 8, 62, 63, 64, 65, 100, 250, 251, 483, 484, 485, 486, 493, 494, 495, 496, 500] if name != 'cond' \
            else [1, 100, 967, 968, 969, 970, 971, 987, 988, 989, 990]
        probe = pygrep_outcomes([f(n) for n in ns])
        for n, hits in zip(ns, probe):
            rx, out = outcome(f(n), 's', 0, pad)
            row = {'shape': name, 'n': n, 'md': 's'}
            row.update(out)
            if rx is not None:
                row['hit'] = [rx.search(x) is not None for x in ('a', 'ab', 'b', '')]
            rows.append(row)
            row = {'shape': name, 'n': n, 'md': 'b'}
            if hits is None:
                row.update({'t': 'RecursionError', 'str': 'maximum recursion depth exceeded'})
            else:
                row.update({'ok': True, 'hit': hits})
            rows.append(row)
    return rows


# --------------------------------------------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------------------------------------------

def dump(path, obj):
    with open(path, 'w', encoding='utf-8', newline='\n') as fh:
        json.dump(obj, fh, ensure_ascii=False, separators=(',', ':'))
        fh.write('\n')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True)
    ap.add_argument('--gen', required=True)
    ap.add_argument('--aliases', required=True)
    ap.add_argument('--refs', default=os.path.expanduser('~/.cache/snp-dev/refs'))
    ap.add_argument('--seed', type=int, default=20261008)
    ap.add_argument('--fuzz-str', type=int, default=20000)
    ap.add_argument('--fuzz-bytes', type=int, default=6000)
    ap.add_argument('--only', default='')
    args = ap.parse_args()

    if sys.version_info[:3] != (3, 12, 3) or sys.implementation.name != 'cpython':
        raise SystemExit('gen_re_vectors.py needs CPython 3.12.3, got %s' % sys.version)
    if unicodedata.unidata_version != '15.0.0':
        raise SystemExit('unexpected UCD %s' % unicodedata.unidata_version)
    os.makedirs(args.out, exist_ok=True)
    os.makedirs(args.gen, exist_ok=True)
    only = set(filter(None, args.only.split(',')))

    def want(x):
        return not only or x in only

    rnd = random.Random(args.seed)
    hay_rnd = random.Random(20261009)  # independent of --seed: the haystack sets stay fixed
    hays = {
        'fuzz_str': str_fuzz_haystacks(hay_rnd, 360),
        'paths': repo_paths(args.refs, 2500, hay_rnd),
        'fuzz_bytes': bytes_fuzz_haystacks(hay_rnd, 360),
        'lines': ref_lines(args.refs, 1500, hay_rnd),
    }
    dump(os.path.join(args.out, 'haystacks.json'), {
        'fuzz_str': hays['fuzz_str'],
        'paths': hays['paths'],
        'fuzz_bytes': [h.hex() for h in hays['fuzz_bytes']],
        'lines': [h.hex() for h in hays['lines']],
    })

    if want('names'):
        aliases, cjk, assigned = names_tables(args.aliases)
        write_names_rs(os.path.join(args.gen, 'names312.rs'), aliases, cjk, assigned)
        dump(os.path.join(args.out, 'names.json'), names_vectors(random.Random(args.seed + 2), aliases))

    if want('classes'):
        dump(os.path.join(args.out, 'classes.json'), class_tables())

    if want('edge'):
        rows = []
        er = random.Random(args.seed + 3)
        for p, mode in edge_patterns():
            set_name = 'fuzz_str' if mode == 's' else 'fuzz_bytes'
            rows.append(vector(p, mode, hays[set_name], set_name, er))
            if mode == 'b':
                for fl in ('i', 'ms'):
                    rows.append(vector(p, mode, hays[set_name], set_name, er, flags=fl, own=False))
        dump(os.path.join(args.out, 'edge.json'), rows)

    if want('corpus'):
        cs, cb = corpus(args.refs)
        cr = random.Random(args.seed + 4)
        rows = [vector(p, 's', hays['paths'], 'paths', cr) for p in cs]
        for p in cb:
            for fl in ('', 'i', 'ms'):
                rows.append(vector(p, 'b', hays['lines'], 'lines', cr, flags=fl))
        dump(os.path.join(args.out, 'corpus.json'), rows)

    if want('nesting'):
        pad = calibrate_pad()
        rows = nesting_rows(pad)
        cap = {(r['n'], r['md']): r.get('ok', False) for r in rows if r['shape'] == 'cap'}
        if not (cap[494, 'b'] and not cap[495, 'b']):
            raise SystemExit('the pygrep probe does not reproduce pygrep\'s 494-group limit')
        dump(os.path.join(args.out, 'nesting.json'), {'pad': pad, 'rows': rows})

    if want('fuzz_str'):
        fr = random.Random(args.seed + 5)
        pats = fuzz_patterns(fr, True, args.fuzz_str)
        rows = [vector(p, 's', hays['fuzz_str'], 'fuzz_str', fr) for p in pats]
        dump(os.path.join(args.out, 'fuzz_str.json'), rows)

    if want('fuzz_bytes'):
        fr = random.Random(args.seed + 6)
        pats = fuzz_patterns(fr, False, args.fuzz_bytes)
        rows = []
        for p in pats:
            fl = fr.choice(['', '', '', 'i', 'ms', 'ims'])
            rows.append(vector(p, 'b', hays['fuzz_bytes'], 'fuzz_bytes', fr, flags=fl))
        dump(os.path.join(args.out, 'fuzz_bytes.json'), rows)


if __name__ == '__main__':
    main()
