"""Generate the Python-parity test vectors in ``tests/vectors/``.

Usage::

    python3.12 -I scripts/gen_parity_vectors.py CFGV_DIR OUT_DIR

``CFGV_DIR`` is a directory containing an unmodified copy of Python cfgv's
``cfgv.py`` (3.5.0); it is the only non-stdlib module imported. The script
must be run with ``-I`` from a directory that contains no ``.py`` files.

Every expected value is computed by CPython itself: ``repr()``/``str()`` of
values, ``str(ValidationError)`` produced by the real cfgv for each schema
case, ``bytes.decode()`` errors, and cfgv's ``load_from_filename`` on files
written to a temporary directory. Output is deterministic (seeded RNG,
sorted keys) so regenerating yields no diff on the same interpreter.

Value encoding (shared by all files)::

    {"t": "none"} | {"t": "bool", "v": true} | {"t": "int", "v": "123"}
    {"t": "float", "bits": "3ff0000000000000"} | {"t": "str", "v": "..."}
    {"t": "bytes", "hex": "00ff"} | {"t": "list" | "tuple" | "set", "v": [...]}
    {"t": "dict", "v": [[key, value], ...]}
    {"t": "date", "v": [y, m, d]}
    {"t": "datetime", "v": [y, m, d, H, M, S, us], "tz": seconds | null}
"""
from __future__ import annotations

import datetime
import json
import math
import os
import random
import re
import struct
import sys
import tempfile
import unicodedata

EXPECTED_PYTHON = (3, 12)
SEED = 20261008


# --------------------------------------------------------------------------
# Value encoding
# --------------------------------------------------------------------------

def enc(v):
    if v is None:
        return {'t': 'none'}
    if isinstance(v, bool):
        return {'t': 'bool', 'v': v}
    if isinstance(v, int):
        return {'t': 'int', 'v': str(v)}
    if isinstance(v, float):
        return {'t': 'float', 'bits': struct.pack('>d', v).hex()}
    if isinstance(v, str):
        return {'t': 'str', 'v': v}
    if isinstance(v, bytes):
        return {'t': 'bytes', 'hex': v.hex()}
    if isinstance(v, list):
        return {'t': 'list', 'v': [enc(x) for x in v]}
    if isinstance(v, tuple):
        return {'t': 'tuple', 'v': [enc(x) for x in v]}
    if isinstance(v, (set, frozenset)):
        # Iteration order of the Python set; the Rust side keeps insertion
        # order, so feeding it this order reproduces Python's repr.
        return {'t': 'set', 'v': [enc(x) for x in v]}
    if isinstance(v, dict):
        return {'t': 'dict', 'v': [[enc(k), enc(x)] for k, x in v.items()]}
    if isinstance(v, datetime.datetime):
        tz = None
        if v.tzinfo is not None:
            tz = int(v.utcoffset().total_seconds())
        return {
            't': 'datetime',
            'v': [v.year, v.month, v.day, v.hour, v.minute, v.second,
                  v.microsecond],
            'tz': tz,
        }
    if isinstance(v, datetime.date):
        return {'t': 'date', 'v': [v.year, v.month, v.day]}
    raise TypeError(f'cannot encode {type(v)}')


def tz(hours=0, minutes=0, negative=False):
    delta = datetime.timedelta(hours=hours, minutes=minutes)
    return datetime.timezone(-delta if negative else delta)


# --------------------------------------------------------------------------
# repr() / str() vectors
# --------------------------------------------------------------------------

CURATED_STRINGS = [
    '', 'a', "it's", 'say "hi"', 'both \'"', 'back\\slash', '^\\.py(',
    'tab\t', 'nl\n', 'cr\r', 'nul\x00', 'bel\x07', '\x1b[31m', 'del\x7f',
    '\x80', '\x9f', '\xa0', '\xad', '\xe9', 'caf\xe9', '\xdf', '\u0378',
    '\u200b', '\u200e', '\u2028', '\u2029', '\u3000', '\ufeff', '\ufffd',
    '\uffff', '\ue000', '\U0001f600', '\U0001fae9', '\U000e0001',
    '\U0010ffff', '\U000f0000', 'C:\\Users\\a b\\python.exe',
    '/tmp/it\'s "x"', "'", '"', '\\', 'a\x0bb\x0cc', 'x\x85y',
    '\U0001f1fa\U0001f1f8', 'e\u0301', '\u0600', '\u061c', '\u1680',
    '\U00013430', '\U0001d173', 'sub/', 'a\tb\u00e9\u200b',
    'foo\\', 'it\'s(', '"\'', "'\"'", '\\\'', 'v1.0', '^(?!.*\\.min\\.js$).*\\.js$',
    '\u2fff', '\u31ef', '\U0002ebf0', '\U00031350', '\U000323b0',
]


def random_char(rng):
    bucket = rng.random()
    if bucket < 0.25:
        return chr(rng.randrange(0, 0x80))
    if bucket < 0.35:
        return rng.choice('\'"\\\t\n\r\x00\x7f')
    if bucket < 0.5:
        return chr(rng.randrange(0x80, 0x800))
    if bucket < 0.8:
        while True:
            cp = rng.randrange(0x800, 0x10000)
            if not 0xD800 <= cp <= 0xDFFF:
                return chr(cp)
    return chr(rng.randrange(0x10000, 0x110000))


def random_strings(rng, n):
    return [
        ''.join(random_char(rng) for _ in range(rng.randrange(0, 10)))
        for _ in range(n)
    ]


def float_cases(rng):
    out = [
        0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.1 + 0.2, 1.1, 1.5, 2.5, 100.0,
        123.456, 1e15, 1e16, 1e17, 1.5e16, 9999999999999998.0,
        9007199254740992.0, 9007199254740993.0, 1e-4, 1e-5, 1.5e-7, 5e-324,
        2.2250738585072014e-308, 1.7976931348623157e308, 1e22, 1e23,
        123456789.0, 1234567890123456789.0, 0.3, 2.0 / 3.0, 1e300, 1e-300,
        float('inf'), float('-inf'), float('nan'), 3.14159, -273.15,
        0.000123, 0.0001234, 12345678901234567.0, 1e100,
    ]
    out += [10.0 ** k for k in range(-30, 31)]
    out += [-(10.0 ** k) for k in range(-6, 20, 3)]
    out += [2.0 ** k for k in range(-60, 70, 7)]
    out += [round(rng.uniform(-1e6, 1e6), rng.randrange(0, 8))
            for _ in range(300)]
    # Exact binary fractions: their decimal expansions end in 5, which
    # makes ties between two equally short candidates likely.
    out += [math.ldexp(1.0, k) for k in range(-1074, 1024)]
    out += [math.ldexp(m, k) for m in range(1, 64, 2) for k in range(-90, 90, 7)]
    while len(out) < 5000:
        bits = rng.getrandbits(64)
        x = struct.unpack('>d', bits.to_bytes(8, 'big'))[0]
        if x == x and x not in (float('inf'), float('-inf')):
            out.append(x)
    return out


def int_cases(rng):
    out = [0, 1, -1, 7, -42, 2 ** 31, 2 ** 53 + 1, 2 ** 63 - 1, -(2 ** 63),
           2 ** 63, -(2 ** 63) - 1, 2 ** 64 - 1, 2 ** 64, 10 ** 20,
           -(10 ** 20), 10 ** 40 + 7, -(10 ** 60) - 1]
    out += [rng.randrange(-(10 ** 30), 10 ** 30) for _ in range(40)]
    return out


def bytes_cases(rng):
    out = [b'', b'ab', b"it's", b'say "hi"', b'both \'"', b'\\', b'\t\n\r',
           b'\x00\x7f\x80\xff', bytes(range(256))]
    out += [bytes(rng.randrange(256) for _ in range(rng.randrange(0, 12)))
            for _ in range(60)]
    return out


def datetime_cases(rng):
    out = [
        datetime.date(2020, 1, 1), datetime.date(1, 1, 1),
        datetime.date(9999, 12, 31), datetime.date(5, 6, 7),
        datetime.datetime(2001, 12, 14, 21, 59, 43, 100000,
                          tzinfo=tz(5, negative=True)),
        datetime.datetime(2001, 12, 14, 0, 0),
        datetime.datetime(2001, 12, 14, 0, 0, 5),
        datetime.datetime(2001, 12, 14, 0, 0, 0, 5),
        datetime.datetime(2002, 1, 1, tzinfo=datetime.timezone.utc),
        datetime.datetime(2002, 1, 1, 1, 2, 3, tzinfo=tz(5, 30)),
        datetime.datetime(2002, 1, 1, 1, 2, 3, tzinfo=tz(0, 30, True)),
        datetime.datetime(2002, 1, 1, 1, 2, 3, tzinfo=tz(23, 59)),
        datetime.datetime(2002, 1, 1, 1, 2, 3, tzinfo=tz(23, 59, True)),
    ]
    for _ in range(40):
        d = datetime.date(rng.randrange(1, 10000), rng.randrange(1, 13),
                          rng.randrange(1, 29))
        out.append(d)
        zone = None
        if rng.random() < 0.6:
            zone = tz(rng.randrange(0, 24), rng.randrange(0, 60),
                      rng.random() < 0.5)
        out.append(datetime.datetime(
            d.year, d.month, d.day, rng.randrange(24), rng.randrange(60),
            rng.choice([0, rng.randrange(60)]),
            rng.choice([0, 0, rng.randrange(1000000)]), tzinfo=zone,
        ))
    return out


def container_cases(rng):
    return [
        [], ['a'], ['a', 'b'], ["it's"], ['a', ['b', ('c',)]], [1, 'a'],
        [True, False, None], [1.0, 1e16, 1.5e-07, -0.0], (), ('a',),
        ('a', 'b'), ((),), ([],), {}, {'a': 1}, {'k': ['v']},
        {1: 'x', 2.5: None, None: (1,), ('t', 1): b'x', True: {'n': []}},
        set(), {1}, {1, 2, 3}, {3, 1, 2}, {-1, 10, 5}, {(1, 2)},
        [datetime.date(2020, 1, 1), {'d': datetime.datetime(2020, 1, 1)}],
        ['pyyaml==6.0', "it's"], ('git', 'diff', '--', 'é'),
        {'nested': {'deeper': [{'x': (1, (2,))}]}},
        [float('nan'), float('inf'), 10 ** 30, b'\x00'],
    ]


def repr_vectors():
    rng = random.Random(SEED)
    values = [None, True, False]
    values += CURATED_STRINGS + random_strings(rng, 400)
    values += float_cases(rng)
    values += int_cases(rng)
    values += bytes_cases(rng)
    values += datetime_cases(rng)
    values += container_cases(rng)
    out = []
    for v in values:
        out.append({
            'value': enc(v),
            'repr': repr(v),
            'str': str(v),
            'type': type(v).__name__,
        })
    return out


def printable_ranges():
    ranges = []
    start = None
    for cp in range(sys.maxunicode + 1):
        printable = chr(cp).isprintable()
        if not printable and start is None:
            start = cp
        elif printable and start is not None:
            ranges.append([start, cp - 1])
            start = None
    if start is not None:
        ranges.append([start, sys.maxunicode])
    return ranges


# --------------------------------------------------------------------------
# UTF-8 vectors
# --------------------------------------------------------------------------

def utf8_vectors():
    rng = random.Random(SEED + 1)
    cases = [
        b'', b'ok', b'\xff', b'\x80', b'a\xc3', b'\xc3(', b'\xe2\x82',
        b'\xe2(\xa1', b'\xe2\x82(', b'\xf0\x9f\x98', b'\xf0(\x8c(',
        b'\xc0\xaf', b'\xc1\xbf', b'\xe0\x80\xaf', b'\xe0\xa0',
        b'\xed\xa0\x80', b'\xf4\x90\x80\x80', b'\xf5\x80',
        b'\xf8\x88\x80\x80\x80', b'ok\xffok', b'abc\xe2\x82', b'\xe2\x82abc',
        b'\xf0\x9f\x98\x80\xff', b'caf\xe9.txt\x00', b'\xef\xbb\xbfbom',
        b'\xa0\xa1', 'é'.encode() * 3 + b'\xe9',
    ]
    alphabet = [b'a', b'\x00', b'\x7f', b'\x80', b'\xbf', b'\xc2', b'\xdf',
                b'\xe0', b'\xed', b'\xef', b'\xf0', b'\xf4', b'\xf5',
                b'\xff', 'é'.encode(), '€'.encode(), '😀'.encode()]
    for _ in range(600):
        cases.append(b''.join(rng.choice(alphabet)
                              for _ in range(rng.randrange(0, 8))))
    out = []
    for b in cases:
        try:
            result = {'ok': b.decode('utf-8')}
        except UnicodeDecodeError as e:
            result = {'error': str(e)}
        out.append({'hex': b.hex(), 'result': result})
    return out


# --------------------------------------------------------------------------
# Schema cases (run through the real cfgv)
# --------------------------------------------------------------------------

class Spec:
    """Builds cfgv objects from JSON specs; the Rust tests mirror this."""

    def __init__(self, cfgv, warnings, regexes):
        self.cfgv = cfgv
        self.warnings = warnings
        self.regexes = regexes

    def value(self, d):
        t = d['t']
        if t == 'none':
            return None
        if t in ('bool', 'str'):
            return d['v']
        if t == 'int':
            return int(d['v'])
        if t == 'float':
            return struct.unpack('>d', bytes.fromhex(d['bits']))[0]
        if t == 'bytes':
            return bytes.fromhex(d['hex'])
        if t == 'list':
            return [self.value(x) for x in d['v']]
        if t == 'tuple':
            return tuple(self.value(x) for x in d['v'])
        if t == 'set':
            return {self.value(x) for x in d['v']}
        if t == 'dict':
            return {self.value(k): self.value(v) for k, v in d['v']}
        if t == 'date':
            return datetime.date(*d['v'])
        if t == 'datetime':
            zone = None
            if d['tz'] is not None:
                zone = datetime.timezone(datetime.timedelta(seconds=d['tz']))
            return datetime.datetime(*d['v'], tzinfo=zone)
        raise ValueError(t)

    def check(self, c):
        cfgv = self.cfgv
        kind = c['kind']
        if kind == 'any':
            return cfgv.check_any
        if kind == 'bool':
            return cfgv.check_bool
        if kind == 'int':
            return cfgv.check_int
        if kind == 'bytes':
            return cfgv.check_bytes
        if kind == 'string':
            return cfgv.check_string
        if kind == 'text':
            return cfgv.check_text
        if kind == 'type':
            tp = TYPES[c['type']]
            return cfgv.check_type(tp, c.get('name'))
        if kind == 'one_of':
            return cfgv.check_one_of(tuple(self.value(v) for v in c['values']))
        if kind == 'regex':
            return self.regex_check
        if kind == 'array':
            return cfgv.check_array(self.check(c['inner']))
        if kind == 'and':
            return cfgv.check_and(*(self.check(x) for x in c['fns']))
        if kind == 'fail':
            def fail(_v, msg=c['msg']):
                raise cfgv.ValidationError(msg)
            return fail
        raise ValueError(kind)

    def regex_check(self, v):
        if isinstance(v, str):
            try:
                re.compile(v)
                self.regexes[v] = True
            except re.error:
                self.regexes[v] = False
        self.cfgv.check_regex(v)

    def condition(self, c):
        kind = c['kind']
        if kind == 'exact':
            return self.value(c['value'])
        if kind == 'not':
            return self.cfgv.Not(self.value(c['value']))
        if kind == 'in':
            return self.cfgv.In(*(self.value(v) for v in c['values']))
        if kind == 'not_in':
            return self.cfgv.NotIn(*(self.value(v) for v in c['values']))
        raise ValueError(kind)

    def item(self, c):
        cfgv = self.cfgv
        kind = c['kind']
        key = self.value(c['key']) if 'key' in c else None
        if kind == 'required':
            return cfgv.Required(key, self.check(c['check']))
        if kind == 'required_recurse':
            return cfgv.RequiredRecurse(key, self.schema(c['schema']))
        if kind == 'optional':
            return cfgv.Optional(key, self.check(c['check']),
                                 self.value(c['default']))
        if kind == 'optional_recurse':
            return cfgv.OptionalRecurse(key, self.schema(c['schema']),
                                        self.value(c['default']))
        if kind == 'optional_no_default':
            return cfgv.OptionalNoDefault(key, self.check(c['check']))
        ck = self.value(c['condition_key']) if 'condition_key' in c else None
        if kind == 'conditional':
            return cfgv.Conditional(
                key, self.check(c['check']), ck,
                self.condition(c['condition']), c['ensure_absent'],
            )
        if kind == 'conditional_optional':
            return cfgv.ConditionalOptional(
                key, self.check(c['check']), self.value(c['default']), ck,
                self.condition(c['condition']), c['ensure_absent'],
            )
        if kind == 'conditional_recurse':
            return cfgv.ConditionalRecurse(
                key, self.schema(c['schema']), ck,
                self.condition(c['condition']), c['ensure_absent'],
            )
        if kind == 'no_additional_keys':
            return cfgv.NoAdditionalKeys(
                tuple(self.value(k) for k in c['keys']))
        if kind == 'warn_additional_keys':
            def callback(extra, keys, dct):
                self.warnings.append(
                    'extra: ' + ', '.join(str(x) for x in extra)
                    + ' | keys: ' + ', '.join(str(x) for x in keys)
                    + f' | size: {len(dct)}',
                )
            return cfgv.WarnAdditionalKeys(
                tuple(self.value(k) for k in c['keys']), callback)
        custom = precommit_item(self, kind, key, c)
        if custom is not None:
            return custom
        raise ValueError(kind)

    def schema(self, s):
        cfgv = self.cfgv
        kind = s['kind']
        if kind == 'map':
            id_key = self.value(s['id_key']) if s['id_key'] else None
            return cfgv.Map(s['name'], id_key,
                            *(self.item(i) for i in s['items']))
        if kind == 'array':
            return cfgv.Array(self.schema(s['of']), s['allow_empty'])
        if kind == 'kvmap':
            return cfgv.KeyValueMap(s['name'], self.check(s['check_key']),
                                    self.schema(s['value_schema']))
        raise ValueError(kind)


# pre-commit's custom map items (pre_commit/clientlib.py, 4.6.2), with
# ``logger.warning`` replaced by appending to the warnings list. The Rust
# tests implement the same items with ``cfgv::MapItem``.
PC_STAGES = ('commit-msg', 'manual', 'pre-commit', 'pre-push')
PC_LEGACY = {'commit': 'pre-commit', 'push': 'pre-push'}


def transform_stage(stage):
    return PC_LEGACY.get(stage, stage)


def translate_language(name):
    return {'system': 'unsupported', 'script': 'unsupported_script'}.get(
        name, name)


def precommit_item(spec, kind, key, c):
    cfgv = spec.cfgv
    warnings = spec.warnings

    class StagesMigration:
        def check(self, dct):
            if key not in dct:
                return
            with cfgv.validate_context(f'At key: {key}'):
                val = dct[key]
                cfgv.check_array(cfgv.check_any)(val)
                val = [transform_stage(v) for v in val]
                cfgv.check_array(cfgv.check_one_of(PC_STAGES))(val)

        def apply_default(self, dct):
            dct.setdefault(key, [])
            dct[key] = [transform_stage(v) for v in dct[key]]

        def remove_default(self, dct):
            pass

    class DeprecatedStagesWarning:
        def check(self, dct):
            if key not in dct:
                return
            val = dct[key]
            cfgv.check_array(cfgv.check_any)(val)
            legacy_stages = [stage for stage in val if stage in PC_LEGACY]
            if legacy_stages:
                warnings.append(
                    f'hook id `{dct["id"]}` uses deprecated stage names '
                    f'({", ".join(legacy_stages)}) which will be removed in '
                    f'a future version.  '
                    f'run: `pre-commit migrate-config` to automatically fix '
                    f'this.',
                )

        def apply_default(self, dct):
            pass

        def remove_default(self, dct):
            pass

    class LanguageMigrationRequired:
        def __init__(self, check_fn):
            self.check_fn = check_fn

        def check(self, dct):
            if key not in dct:
                raise cfgv.ValidationError(f'Missing required key: {key}')
            with cfgv.validate_context(f'At key: {key}'):
                self.check_fn(translate_language(dct[key]))

        def apply_default(self, dct):
            if key in dct:
                dct[key] = translate_language(dct[key])

        def remove_default(self, dct):
            pass

    class WarnMutableRev(cfgv.Conditional):
        def check(self, dct):
            super().check(dct)
            if self.key in dct:
                rev = dct[self.key]
                if '.' not in rev and not re.match(r'^[a-fA-F0-9]+$', rev):
                    warnings.append(
                        f'The {self.key!r} field of repo {dct["repo"]!r} '
                        f'appears to be a mutable reference '
                        f'(moving tag / branch).  Mutable references are '
                        f'never updated after first install and are not '
                        f'supported.  '
                        f'See https://pre-commit.com/#using-the-latest-version-for-a-repository '  # noqa: E501
                        f'for more details.  '
                        f'Hint: `pre-commit autoupdate` often fixes this.',
                    )

    class OptionalSensibleRegexAtHook(cfgv.OptionalNoDefault):
        def check(self, dct):
            super().check(dct)
            if '/*' in dct.get(self.key, ''):
                warnings.append(
                    f'The {self.key!r} field in hook {dct.get("id")!r} is a '
                    f"regex, not a glob -- matching '/*' probably isn't "
                    f'what you want here',
                )
            for fwd_slash_re in (r'[\\/]', r'[\/]', r'[/\\]'):
                if fwd_slash_re in dct.get(self.key, ''):
                    warnings.append(
                        fr'pre-commit normalizes slashes in the {self.key!r} '
                        fr'field in hook {dct.get("id")!r} to forward '
                        fr'slashes, so you can use / instead of '
                        fr'{fwd_slash_re}',
                    )

    class NotAllowed(cfgv.OptionalNoDefault):
        def check(self, dct):
            if self.key in dct:
                raise cfgv.ValidationError(f'{self.key!r} cannot be overridden')

    if kind == 'pc_stages_migration':
        return StagesMigration()
    if kind == 'pc_deprecated_stages_warning':
        return DeprecatedStagesWarning()
    if kind == 'pc_language_migration_required':
        return LanguageMigrationRequired(spec.check(c['check']))
    if kind == 'pc_warn_mutable_rev':
        return WarnMutableRev(
            key, cfgv.check_string, spec.value(c['condition_key']),
            spec.condition(c['condition']), True,
        )
    if kind == 'pc_sensible_regex':
        return OptionalSensibleRegexAtHook(key, cfgv.check_string)
    if kind == 'pc_not_allowed':
        return NotAllowed(key, cfgv.check_any)
    return None


TYPES = {
    'bool': bool, 'int': int, 'float': float, 'str': str, 'bytes': bytes,
    'list': list, 'tuple': tuple, 'dict': dict, 'set': set,
    'NoneType': type(None), 'date': datetime.date,
    'datetime': datetime.datetime,
}

S = {'t': 'str'}


def s(v):
    return {'t': 'str', 'v': v}


def i(v):
    return {'t': 'int', 'v': str(v)}


def fl(v):
    return {'t': 'float', 'bits': struct.pack('>d', v).hex()}


def b(v):
    return {'t': 'bool', 'v': v}


NONE = {'t': 'none'}


def lst(*v):
    return {'t': 'list', 'v': list(v)}


def tup(*v):
    return {'t': 'tuple', 'v': list(v)}


def dct(*pairs):
    return {'t': 'dict', 'v': [list(p) for p in pairs]}


def check(kind, **kw):
    return {'kind': kind, **kw}


def mp(name, id_key, *items):
    return {'kind': 'map', 'name': name, 'id_key': id_key,
            'items': list(items)}


def item(kind, **kw):
    return {'kind': kind, **kw}


SAMPLE_VALUES = [
    NONE, b(True), b(False), i(0), i(10 ** 30), fl(1.0), fl(1e16),
    fl(float('nan')), s(''), s("it's"), s('a\\b'), s('tab\tx'),
    {'t': 'bytes', 'hex': '6869'}, lst(), lst(s('a')), tup(), tup(s('a')),
    tup(s('a'), i(1)), dct(), dct((s('k'), s('v'))),
    {'t': 'set', 'v': [i(1), i(2)]}, {'t': 'set', 'v': []},
    {'t': 'date', 'v': [2020, 1, 1]},
    {'t': 'datetime', 'v': [2020, 1, 1, 10, 0, 0, 0], 'tz': None},
    {'t': 'datetime', 'v': [2001, 12, 14, 21, 59, 43, 100000],
     'tz': -18000},
]


def schema_cases():
    cases = []

    def add(name, schema, value):
        cases.append({'name': name, 'schema': schema, 'value': value})

    # check_type and friends against every kind of value.
    type_checks = [
        ('bool', check('bool')), ('int', check('int')),
        ('bytes', check('bytes')), ('string', check('string')),
        ('text', check('text')),
    ] + [
        (f'type_{t}', check('type', type=t)) for t in TYPES
    ] + [
        ('type_named', check('type', type='dict', name='mapping')),
    ]
    for label, c in type_checks:
        for n, v in enumerate(SAMPLE_VALUES):
            add(f'{label}_{n}', mp('M', None, item('required', key=s('k'),
                                                    check=c)),
                dct((s('k'), v)))

    # check_one_of: choices are sorted with sorted() and shown with str().
    one_ofs = [
        [i(10), i(2), i(1)], [s('b'), s('a'), s('C'), s('é')],
        [fl(2.5), i(1), b(True), i(-3)], [s("it's"), s('x')],
        [i(10 ** 30), i(5), fl(1e20)],
        [tup(i(2), i(1)), tup(i(1), i(3))],
    ]
    for n, choices in enumerate(one_ofs):
        for m, v in enumerate(SAMPLE_VALUES + [i(1), fl(1.0), s('a')]):
            add(f'one_of_{n}_{m}',
                mp('M', None, item('required', key=s('k'),
                                   check=check('one_of', values=choices))),
                dct((s('k'), v)))

    # check_regex: the message is the repr of the pattern.
    patterns = ['^$', '(', "it's(", 'say "hi"(', 'both \'"(', 'foo\\',
                'tab\t(', '[', 'a\\Z', '(?<!foo)bar', '(?P<n>x)(?P=n)',
                'é(', '\U0001f600)', '\x00(', '*', 'a{', '\\', '^a\\.txt$']
    for n, p in enumerate(patterns):
        add(f'regex_{n}',
            mp('M', None, item('required', key=s('k'), check=check(
                'and', fns=[check('string'), check('regex')]))),
            dct((s('k'), s(p))))

    # check_array: non-arrays and nested indexes.
    for n, v in enumerate(SAMPLE_VALUES):
        add(f'array_{n}', mp('M', None, item('required', key=s('k'), check=check(
            'array', inner=check('array', inner=check('string'))))),
            dct((s('k'), v)))
    add('array_nested_index',
        mp('M', None, item('required', key=s('k'), check=check(
            'array', inner=check('array', inner=check('string'))))),
        dct((s('k'), lst(lst(s('a')), tup(s('b'), i(1))))))

    # Map: non-dicts, id key repr, MISSING.
    for n, v in enumerate(SAMPLE_VALUES):
        add(f'map_not_dict_{n}', mp('Hook', s('id')), v)
        add(f'map_id_{n}',
            mp('Hook', s('id'), item('required', key=s('name'),
                                     check=check('string'))),
            dct((s('id'), v)))
    add('map_id_missing',
        mp('Hook', s('id'), item('required', key=s('id'),
                                 check=check('string'))),
        dct())
    add('map_no_id', mp('Config', None, item('required', key=s('x'),
                                             check=check('any'))),
        dct())
    add('map_int_id_key',
        mp('Thing', i(5), item('required', key=i(5), check=check('string'))),
        dct((i(5), i(6))))

    # Required / Optional with keys of several types (str() of the key).
    for n, key in enumerate([s('key'), i(1), b(True), NONE, fl(2.5),
                             s("it's"), tup(s('a'), i(1))]):
        add(f'required_missing_{n}',
            mp('M', None, item('required', key=key, check=check('bool'))),
            dct())
        add(f'required_bad_{n}',
            mp('M', None, item('required', key=key, check=check('bool'))),
            dct((key, i(1))))
        add(f'optional_bad_{n}',
            mp('M', None, item('optional', key=key, check=check('bool'),
                               default=b(False))),
            dct((key, s('x'))))
        add(f'optional_no_default_bad_{n}',
            mp('M', None, item('optional_no_default', key=key,
                               check=check('bool'))),
            dct((key, s('x'))))

    # Nested schemas and contexts.
    inner = mp('Inner', s('name'),
               item('required', key=s('name'), check=check('string')),
               item('optional', key=s('flag'), check=check('bool'),
                    default=b(False)))
    outer = mp('Outer', None,
               item('required_recurse', key=s('inner'), schema=inner),
               item('optional_recurse', key=s('opt'), schema=inner,
                    default=dct((s('name'), s('dflt')))),
               item('optional', key=s('n'), check=check('int'), default=i(0)),
               item('optional', key=s('z'), check=check('any'),
                    default=lst()))
    for n, v in enumerate([
        dct(),
        dct((s('inner'), i(1))),
        dct((s('inner'), dct())),
        dct((s('inner'), dct((s('name'), i(1))))),
        dct((s('inner'), dct((s('name'), s('x')), (s('flag'), s('no'))))),
        dct((s('inner'), dct((s('name'), s('x')))),
            (s('opt'), dct((s('name'), b(True))))),
        dct((s('inner'), dct((s('name'), s('x')))), (s('n'), i(0)),
            (s('z'), lst()), (s('opt'), dct((s('name'), s('dflt'))))),
        dct((s('z'), lst()), (s('inner'), dct((s('flag'), b(False)),
                                              (s('name'), s('x')))),
            (s('n'), i(3))),
        dct((s('inner'), dct((s('name'), s('x')))),
            (s('opt'), dct((s('name'), s('o')), (s('flag'), b(False))))),
        dct((s('n'), b(True)), (s('inner'), dct((s('name'), s('y'))))),
    ]):
        add(f'nested_{n}', outer, v)

    # Arrays of maps (no index context), allow_empty.
    hooks = {'kind': 'array', 'of': inner, 'allow_empty': True}
    hooks_ne = {'kind': 'array', 'of': inner, 'allow_empty': False}
    for n, v in enumerate([lst(), tup(), lst(dct((s('name'), s('a'))),
                                             dct((s('name'), i(1)))),
                           dct(), s('x'), NONE,
                           tup(dct((s('name'), s('a')))),
                           lst(dct((s('name'), s('a')), (s('flag'), b(False))))]):
        add(f'array_schema_{n}', hooks, v)
        add(f'array_schema_nonempty_{n}', hooks_ne, v)

    # Conditionals.
    conditions = [
        {'kind': 'exact', 'value': s('local')},
        {'kind': 'exact', 'value': i(1)},
        {'kind': 'exact', 'value': NONE},
        {'kind': 'exact', 'value': b(True)},
        {'kind': 'not', 'value': s("it's")},
        {'kind': 'not', 'value': fl(1.0)},
        {'kind': 'in', 'values': [s('a')]},
        {'kind': 'in', 'values': [s('a'), i(2)]},
        {'kind': 'not_in', 'values': [s('local'), s('meta')]},
        {'kind': 'not_in', 'values': [s('only')]},
        {'kind': 'not_in', 'values': []},
    ]
    condition_values = [None, s('local'), s('meta'), s('a'), s("it's"),
                        i(1), fl(1.0), b(True), i(2), NONE, s('other'),
                        s('only')]
    found_values = [s('v1.0'), fl(1.0), i(5), lst(s('x')), NONE,
                    s("it's")]
    for n, cond in enumerate(conditions):
        for ensure_absent in (False, True):
            for m, cv in enumerate(condition_values):
                for k, fv in enumerate(found_values[:3] if m else found_values):
                    pairs = []
                    if cv is not None:
                        pairs.append((s('repo'), cv))
                    pairs.append((s('rev'), fv))
                    value = dct(*pairs)
                    tag = f'{n}_{int(ensure_absent)}_{m}_{k}'
                    add(f'conditional_{tag}',
                        mp('Repo', s('repo'), item(
                            'conditional', key=s('rev'),
                            check=check('string'), condition_key=s('repo'),
                            condition=cond, ensure_absent=ensure_absent)),
                        value)
                    add(f'conditional_optional_{tag}',
                        mp('Repo', s('repo'), item(
                            'conditional_optional', key=s('rev'),
                            check=check('string'), default=s('HEAD'),
                            condition_key=s('repo'), condition=cond,
                            ensure_absent=ensure_absent)),
                        value)
                    add(f'conditional_recurse_{tag}',
                        mp('Repo', s('repo'), item(
                            'conditional_recurse', key=s('rev'),
                            schema=inner, condition_key=s('repo'),
                            condition=cond, ensure_absent=ensure_absent)),
                        value)
            # Missing conditional key altogether.
            add(f'conditional_missing_{n}_{int(ensure_absent)}',
                mp('Repo', s('repo'), item(
                    'conditional', key=s('rev'), check=check('string'),
                    condition_key=s('repo'), condition=cond,
                    ensure_absent=ensure_absent)),
                dct((s('repo'), s('local'))))
    # ConditionalOptional / ConditionalRecurse defaults.
    add('conditional_optional_defaults',
        mp('M', None,
           item('conditional_optional', key=s('rev'), check=check('string'),
                default=s('HEAD'), condition_key=s('repo'),
                condition={'kind': 'not', 'value': s('local')},
                ensure_absent=False),
           item('optional', key=s('after'), check=check('any'),
                default=i(1))),
        dct((s('repo'), s('x'))))
    add('conditional_recurse_defaults',
        mp('M', None,
           item('conditional_recurse', key=s('sub'), schema=inner,
                condition_key=s('kind'),
                condition={'kind': 'exact', 'value': s('a')},
                ensure_absent=False)),
        dct((s('kind'), s('a')), (s('sub'), dct((s('name'), s('n'))))))

    # NoAdditionalKeys / WarnAdditionalKeys.
    for n, v in enumerate([
        dct((s('b'), i(1)), (s('a'), i(2)), (s('z'), i(3)), (s('B'), i(4))),
        dct((s('ok'), i(1))),
        dct((i(10), i(1)), (i(2), i(1)), (fl(1.5), i(0))),
        dct((b(True), i(1)), (s('ok'), i(1))),
        dct((s("it's"), i(1)), (s('é'), i(1)), (s('ok'), i(1))),
        dct((tup(i(2)), i(1)), (tup(i(1), i(5)), i(1))),
    ]):
        add(f'no_additional_keys_{n}',
            mp('M', None, item('no_additional_keys',
                               keys=[s('ok'), i(1), s('also')])),
            v)
        add(f'warn_additional_keys_{n}',
            mp('M', None,
               item('warn_additional_keys', keys=[s('ok'), i(1)]),
               item('optional', key=s('ok'), check=check('int'),
                    default=i(0)),
               item('warn_additional_keys', keys=[s('ok')])),
            v)

    # KeyValueMap.
    kv = {'kind': 'kvmap', 'name': 'Env',
          'check_key': check('string'), 'value_schema': inner}
    for n, v in enumerate([
        dct(), dct((s('a'), dct((s('name'), s('x'))))),
        dct((s('a'), dct((s('name'), s('x')))), (i(1), dct())),
        dct((s('it\'s'), dct((s('name'), i(1))))),
        lst(), NONE, s('x'),
    ]):
        add(f'kvmap_{n}', kv, v)

    # Custom check failures carry contexts.
    add('custom_fail',
        mp('M', s('id'),
           item('required', key=s('id'), check=check('any')),
           item('optional', key=s('x'), check=check('array', inner=check(
               'fail', msg='Type tag \'pyproject\' is not recognized.  '
                           'Try upgrading identify and pre-commit?')),
                default=lst())),
        dct((s('id'), s('h')), (s('x'), lst(s('a'), s('b')))))

    # Python numeric equality in conditions, one_of and key sets.
    add('numeric_eq_condition',
        mp('M', None, item('conditional', key=s('x'), check=check('string'),
                           condition_key=s('c'),
                           condition={'kind': 'exact', 'value': b(True)},
                           ensure_absent=True)),
        dct((s('c'), fl(1.0))))
    # pre-commit-like configuration with custom items and warnings.
    pc_hook = mp(
        'Hook', s('id'),
        item('required', key=s('id'), check=check('string')),
        item('pc_language_migration_required', key=s('language'),
             check=check('one_of', values=[
                 s('python'), s('unsupported'), s('unsupported_script')])),
        item('pc_stages_migration', key=s('stages')),
        item('pc_sensible_regex', key=s('files')),
        item('pc_deprecated_stages_warning', key=s('stages')),
        item('optional', key=s('always_run'), check=check('bool'),
             default=b(False)),
    )
    pc_meta_hook = mp(
        'Hook', s('id'),
        item('required', key=s('id'), check=check('string')),
        item('pc_not_allowed', key=s('entry')),
    )
    pc_repo = mp(
        'Repository', s('repo'),
        item('required', key=s('repo'), check=check('string')),
        item('pc_warn_mutable_rev', key=s('rev'), condition_key=s('repo'),
             condition={'kind': 'not_in', 'values': [s('local'), s('meta')]}),
        item('conditional_recurse', key=s('hooks'),
             schema={'kind': 'array', 'of': pc_hook, 'allow_empty': True},
             condition_key=s('repo'),
             condition={'kind': 'not', 'value': s('meta')},
             ensure_absent=False),
        item('conditional_recurse', key=s('hooks'),
             schema={'kind': 'array', 'of': pc_meta_hook,
                     'allow_empty': True},
             condition_key=s('repo'),
             condition={'kind': 'exact', 'value': s('meta')},
             ensure_absent=False),
        item('warn_additional_keys', keys=[s('repo'), s('rev'), s('hooks')]),
    )
    pc_config = mp(
        'Config', None,
        item('required_recurse', key=s('repos'),
             schema={'kind': 'array', 'of': pc_repo, 'allow_empty': True}),
        item('optional', key=s('fail_fast'), check=check('bool'),
             default=b(False)),
        item('warn_additional_keys', keys=[s('repos'), s('fail_fast')]),
    )

    def hook(hid, *pairs):
        return dct((s('id'), s(hid)), *pairs)

    def repo(url, *pairs):
        return dct((s('repo'), s(url)), *pairs)

    pc_values = [
        dct((s('repos'), lst())),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('v1.0')),
                                  (s('hooks'), lst()))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('main')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('python'))))))))),
        dct((s('repos'), lst(
            repo('https://r', (s('rev'), s('deadBEEF')), (s('hooks'), lst(
                hook('a', (s('language'), s('system')),
                     (s('stages'), lst(s('commit'), s('push'), s('manual'))),
                     (s('files'), s('^src/*.py'))),
                hook('b', (s('language'), s('script')),
                     (s('files'), s(r'a[\\/]b[/\\]c[\/]'))),
            ))),
            repo('local', (s('hooks'), lst(hook('c', (s('language'), s('python')),
                                                (s('always_run'), b(True)))))),
            repo('meta', (s('hooks'), lst(hook('check-hooks-apply')))),
        )), (s('fail_fast'), b(True)), (s('extra_root'), i(1))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('topic')),
                                  (s('hooks'), lst(hook('a'))))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('topic')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('ruby'))))))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('v1')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('python')),
                                                        (s('stages'), lst(s('commit'), s('bogus')))))))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('v1')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('python')),
                                                        (s('stages'), s('commit'))))))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('v1')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('python')),
                                                        (s('files'), i(1))))))))),
        dct((s('repos'), lst(repo('local', (s('rev'), s('v1.0')))))),
        dct((s('repos'), lst(repo('meta', (s('hooks'), lst(
            hook('identity', (s('entry'), s('x'))))))))),
        dct((s('repos'), lst(repo('https://r', (s('rev'), s('v1')),
                                  (s('hooks'), lst(hook('a', (s('language'), s('python')),
                                                        (s('stages'), lst(s('commit')))))),
                                  (s('unknown'), i(1)), (s('also'), i(2))),
                       repo('https://x'))), (s('zzz'), i(1)), (s('aaa'), i(1))),
    ]
    for n, v in enumerate(pc_values):
        add(f'precommit_{n}', pc_config, v)

    add('numeric_eq_remove_default',
        mp('M', None, item('optional', key=s('x'), check=check('any'),
                           default=i(0))),
        dct((s('x'), b(False))))
    return cases


def run_schema_cases(cfgv, cases, regexes):
    out = []
    for case in cases:
        warnings = []
        spec = Spec(cfgv, warnings, regexes)
        schema = spec.schema(case['schema'])
        value = spec.value(case['value'])
        try:
            cfgv.validate(value, schema)
        except cfgv.ValidationError as e:
            result = {'error': str(e)}
        else:
            result = {
                'applied': repr(cfgv.apply_defaults(value, schema)),
                'removed': repr(cfgv.remove_defaults(value, schema)),
            }
        out.append({**case, 'result': result, 'warnings': warnings})
    return out


# --------------------------------------------------------------------------
# load_from_filename cases
# --------------------------------------------------------------------------

def kv_loader(cfgv):
    """``key=value`` lines -> dict; mirrored by the Rust tests."""
    def load(contents):
        out = {}
        for line in contents.split('\n'):
            if not line:
                continue
            if line == 'NESTED':
                raise cfgv.ValidationError('nested failure')
            if '=' not in line:
                raise ValueError(f'bad line: {line!r}')
            k, _, v = line.partition('=')
            out[k] = v
        return out
    return load


class LoadFailed(Exception):
    """Like pre-commit's InvalidConfigError: ``str()`` is ``str(e)``."""


def load_cases(cfgv, tmp):
    schema = cfgv.Map(
        'Config', None,
        cfgv.Required('name', cfgv.check_string),
        cfgv.Optional('mode', cfgv.check_one_of(('a', 'b')), 'a'),
    )
    schema_json = mp(
        'Config', None,
        item('required', key=s('name'), check=check('string')),
        item('optional', key=s('mode'),
             check=check('one_of', values=[s('a'), s('b')]),
             default=s('a')),
    )
    big = b'name=' + b'x' * 10000 + b'\nmode=\xe9\n'
    contents = [
        b'name=x\n', b'name=x\nmode=b\n', b'name=x\r\nmode=b\r\n',
        b'mode=a\rname=y\r', b'mode=c\nname=x\n', b'', b'\xff', b'name=\xc3',
        b'name=caf\xc3\xa9', b'name=caf\xe9\n', b'garbage line\n',
        b"it's bad\n", b'name=x\nNESTED\n', b'\xef\xbb\xbfname=x\n',
        b'name=a\r\r\nmode=b', big, b'name=x\n' + b'\x80' * 3,
    ]
    out = []
    load = kv_loader(cfgv)
    for n, data in enumerate(contents):
        path = os.path.join(tmp, f'case{n}.cfg')
        with open(path, 'wb') as f:
            f.write(data)
        for display in (None, f'shown{n}.cfg', ''):
            try:
                ret = cfgv.load_from_filename(path, schema, load,
                                              exc_tp=LoadFailed,
                                              display_filename=display)
            except LoadFailed as e:
                result = {'error': str(e).replace(tmp, '$TMP')}
            else:
                result = {'applied': repr(ret)}
            out.append({'name': f'load_{n}_{display}', 'hex': data.hex(),
                        'file': f'case{n}.cfg', 'display': display,
                        'schema': schema_json, 'result': result})
    for name in ('missing.cfg', 'a_directory'):
        if name == 'a_directory':
            os.mkdir(os.path.join(tmp, name))
        for display in (None, 'shown.cfg'):
            path = os.path.join(tmp, name)
            try:
                cfgv.load_from_filename(path, schema, load,
                                        exc_tp=LoadFailed,
                                        display_filename=display)
            except LoadFailed as e:
                result = {'error': str(e).replace(tmp, '$TMP')}
            out.append({'name': f'{name}_{display}', 'hex': None,
                        'file': name, 'display': display,
                        'schema': schema_json, 'result': result})
    return out


# --------------------------------------------------------------------------

def dump_lines(data):
    """JSON with one list element per line (diff-friendly, compact)."""
    def compact(v):
        return json.dumps(v, ensure_ascii=True, sort_keys=True,
                          separators=(',', ':'))
    parts = []
    for key in sorted(data):
        v = data[key]
        if isinstance(v, list):
            body = ',\n'.join(compact(x) for x in v)
            parts.append(f'{compact(key)}:[\n{body}\n]')
        else:
            parts.append(f'{compact(key)}:{compact(v)}')
    return '{\n' + ',\n'.join(parts) + '\n}\n'


def main():
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    if sys.version_info[:2] != EXPECTED_PYTHON:
        print(f'expected CPython {EXPECTED_PYTHON}', file=sys.stderr)
        return 1
    cfgv_dir, out_dir = sys.argv[1], sys.argv[2]
    sys.path.insert(0, os.path.abspath(cfgv_dir))
    import cfgv  # noqa: E402  (the copied original, nothing else)

    meta = {
        'python': '.'.join(map(str, sys.version_info[:2])),
        'unicode': unicodedata.unidata_version,
        'cfgv': '3.5.0',
        'seed': SEED,
    }

    regexes = {}
    schema = run_schema_cases(cfgv, schema_cases(), regexes)
    with tempfile.TemporaryDirectory() as tmp:
        load = load_cases(cfgv, tmp)

    schemas = []
    index = {}

    def intern(case):
        key = json.dumps(case['schema'], sort_keys=True)
        if key not in index:
            index[key] = len(schemas)
            schemas.append(case['schema'])
        return {**case, 'schema': index[key]}

    schema = [intern(c) for c in schema]
    load = [intern(c) for c in load]
    files = {
        'repr.json': {'meta': meta, 'cases': repr_vectors(),
                      'nonprintable': printable_ranges()},
        'utf8.json': {'meta': meta, 'cases': utf8_vectors()},
        'messages.json': {'meta': meta, 'regex_valid': regexes,
                          'schemas': schemas, 'cases': schema,
                          'load': load},
    }
    for name, data in files.items():
        with open(os.path.join(out_dir, name), 'w', encoding='utf-8',
                  newline='\n') as f:
            f.write(dump_lines(data))
        print(f'wrote {name}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
