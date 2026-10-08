"""Render selected golden vectors as markdown tables (values shown as Python repr).

    python3 -I render_pycompat_tables.py VECTORS.json OUT.md
"""
import json
import sys

V = json.load(open(sys.argv[1], encoding='utf-8'))
out = []


def code(x) -> str:
    s = x if isinstance(x, str) else repr(x)
    if s == '':
        return '*(empty string)*'
    s = s.replace('|', '\\|')
    if '`' in s:
        return f'`` {s} ``'
    return f'`{s}`'


def r(x):
    return code(repr(x))


def b(h):
    return bytes.fromhex(h)


def res(o):
    if 'ok' in o:
        return r(o['ok'] if not isinstance(o['ok'], list) else o['ok'])
    return code(f"{o['error']}: {o['msg']}")


def table(title, header, rows):
    out.append(f'#### {title}\n')
    out.append('| ' + ' | '.join(header) + ' |')
    out.append('|' + '---|' * len(header))
    for row in rows:
        out.append('| ' + ' | '.join(row) + ' |')
    out.append('')


def pick(sec, pred=lambda e: True, limit=None):
    xs = [e for e in V[sec] if pred(e)]
    return xs[:limit] if limit else xs


Z, O1, O2 = '0' * 40, '1' * 40, '2' * 40


def shas(s):
    return s.replace(O1, '<O1>').replace(O2, '<O2>').replace(Z, '<Z40>')


# ---- splitlines
sl = {}
for e in V['str_splitlines']:
    sl.setdefault(e['in'], {})[e['keepends']] = e['out']
table('V-SL: `str.splitlines()` / `splitlines(True)` (section `str_splitlines`)',
      ['input', 'splitlines()', 'splitlines(True)'],
      [[r(k), r(v[False]), r(v[True])] for k, v in sl.items()
       if k not in ('a\u2029b', 'a\x1db', 'a\x1eb', 'a\r', 'no newline at end', '\u2028')])
bl = {}
for e in V['bytes_splitlines']:
    bl.setdefault(e['in_hex'], {})[e['keepends']] = [b(x) for x in e['out_hex']]
table('V-BSL: `bytes.splitlines()` (section `bytes_splitlines`)',
      ['input', 'splitlines()', 'splitlines(True)'],
      [[r(b(k)), r(v[False]), r(v[True])] for k, v in bl.items()])

# ---- split / rsplit
table('V-SPLIT: `str.split()` (sep=None)', ['input', 'result'],
      [[r(e['in']), r(e['out'])] for e in V['str_split_none']])
table('V-SPLITN: `str.split(None, maxsplit)`', ['input', 'maxsplit', 'result'],
      [[r(e['in']), str(e['maxsplit']), r(e['out'])] for e in V['str_split_none_maxsplit']
       if e['in'] not in ('',)])
table('V-RSPLIT: `str.rsplit(None, 3)` (pre-push field split)', ['input', 'rsplit(maxsplit=3)'],
      [[r(shas(e['in'])), r([shas(x) for x in e['out']])] for e in V['str_rsplit_none_maxsplit']
       if e['maxsplit'] == 3])
table('V-SEP: `str.split(sep)` (== Rust `str::split(sep)` for non-empty sep)', ['input', 'sep', 'result'],
      [[r(e['in']), r(e['sep']), r(e['out'])] for e in V['str_split_sep']
       if e['in'] in ('', ',', ' a , b ', 'a,,b', '/a::/b:', ';.EXE', 'a\x00b\x00', '1..2')])
table('V-STRIP: `str.strip()/lstrip()/rstrip()`', ['input', 'strip()', 'lstrip()', 'rstrip()'],
      [[r(e['in']), r(e['strip']), r(e['lstrip']), r(e['rstrip'])] for e in V['str_strip_none']])
table('V-STRIPC: `str.strip(chars)` (chars is a set)', ['input', 'chars', 'strip', 'lstrip', 'rstrip'],
      [[r(e['in']), r(e['chars']), r(e['strip']), r(e['lstrip']), r(e['rstrip'])] for e in V['str_strip_chars']])
table('V-BSTRIP: `bytes.strip()` and `bytes.strip(set)`', ['input', 'chars', 'strip', 'lstrip', 'rstrip'],
      [[r(b(e['in_hex'])), '(None)', r(b(e['strip_hex'])), r(b(e['lstrip_hex'])), r(b(e['rstrip_hex']))]
       for e in V['bytes_strip_none']] +
      [[r(b(e['in_hex'])), r(b(e['chars_hex'])), r(b(e['strip_hex'])), r(b(e['lstrip_hex'])), r(b(e['rstrip_hex']))]
       for e in V['bytes_strip_chars']])

# ---- composites
table('V-ZSPLIT: `git.zsplit` (`git.py:19-24`)', ['input', 'result'],
      [[r(e['in']), r(e['out'])] for e in V['zsplit']])
table('V-SKIP: `SKIP` parsing (`run.py:129-131`), set shown sorted', ['SKIP', 'set'],
      [[r(e['in']), r(e['out_sorted_set'])] for e in V['skip_env']])
rows = []
for e in V['pre_push_parse']:
    o = e['out']
    if 'decode' in o:
        res_s = code('UnicodeDecodeError: ' + o['decode']['msg'])
    else:
        parts = []
        for ln in o['lines']:
            if 'fields' in ln:
                parts.append(repr([shas(x) for x in ln['fields']]))
            else:
                parts.append(f"ValueError: {ln['msg']}")
        res_s = code(' ; '.join(parts) if parts else '(no lines)')
    rows.append([r(shas(b(e['in_hex']).decode('utf-8', 'backslashreplace'))), res_s])
table('V-PP: pre-push stdin parsing (`hook_impl.py:128-130`; `<O1>`/`<O2>` = 40×`1`/`2`, `<Z40>` = 40×`0`; input shown decoded with backslashreplace)',
      ['stdin bytes', 'per line: fields, or the exception that pre-commit raises (exit 3)'], rows)
table('V-MM: `parse_merge_msg_for_conflicts` (`git.py:104-111`)', ['MERGE_MSG bytes', 'result'],
      [[r(b(e['in_hex'])), res(e['out'])] for e in V['merge_msg_conflicts']])
table('V-REV: `REV_LINE_RE.match(line).groups()` (`autoupdate.py:116`)', ['line', 'groups 1..6 (None = no match)'],
      [[r(e['in']), r(e['groups'] if e['groups'] is None else tuple(e['groups']))] for e in V['rev_line_re']])
table('V-REVW: rev-line rewrite (`autoupdate.py:146-156`; `new_rev` = rendered YAML scalar)',
      ['line', 'new_rev', 'frozen', 'result'],
      [[r(e['line']), r(e['new_rev']), r(e['frozen']), r(e['out'])] for e in V['rev_line_rewrite']])
table('V-AUL: `_original_lines` rev-line detection (`autoupdate.py:120-130`)', ['file text (newline=\'\')', 'lines', 'rev idxs'],
      [[r(e['in']), r(e['out']['lines']), r(e['out']['idxs'])] for e in V['autoupdate_original_lines']])
table('V-MIG: `_migrate_map` header split + `textwrap.indent(rest, "    ")` (`migrate_config.py:21-44`)',
      ['contents', 'header', 'rest', 'indented rest'],
      [[r(e['in']), r(e['out']['header']), r(e['out']['rest']), r(e['out']['indented_rest'])]
       for e in V['migrate_header_split']])
table('V-TIO: text-mode read (`newline=None` vs `newline=\'\'`)', ['file bytes', 'newline=None', "newline=''"],
      [[r(b(e['in_hex'])), r(e['newline_none']), r(e['newline_empty'])] for e in V['text_read']])
table('V-TIOW: text-mode write (`open(f, "w")`)', ['str', 'POSIX bytes', 'Windows bytes (os.linesep)'],
      [[r(e['in']), r(b(e['posix_hex'])), r(b(e['windows_hex']))] for e in V['text_write']])
table('V-UTF8: `bytes.decode()` (strict) — `str(UnicodeDecodeError)`', ['bytes', 'result'],
      [[r(b(e['in_hex'])), res(e['out'])] for e in V['utf8_decode_strict']])
table("V-BSR: `bytes.decode(errors='backslashreplace')`", ['bytes', 'result'],
      [[r(b(e['in_hex'])), r(e['out'])] for e in V['utf8_decode_backslashreplace']])


def untag(t):
    k = t['t']
    if k == 'str':
        return t['v'] if 'v' in t else ''.join(map(chr, t['v_cps']))
    if k == 'list':
        return [untag(x) for x in t['v']]
    if k == 'tuple':
        return tuple(untag(x) for x in t['v'])
    if k == 'dict':
        return {untag(a): untag(c) for a, c in t['v']}
    if k == 'none':
        return None
    if k == 'float':
        return float(t['v'])
    return t['v']


table('V-REPR: `repr()` (input shown as `ascii()` to make it unambiguous)', ['input (ascii())', 'repr() output'],
      [[code(ascii(untag(e['in']))), code(e['out'])] for e in V['repr']])
table("V-FEQ: `f'{prefix=}'` (`hazmat.py:51`)", ['prefix', 'rendered'],
      [[r(untag(e['in'])), code(e['out'])] for e in V['fstring_eq']])
table('V-OSE: `str(OSError(*args))`', ['args', 'str()'],
      [[r(tuple(e['args'])), code(e['out'])] for e in V['oserror_str']])
table('V-CPE: `str(subprocess.CalledProcessError(rc, cmd))` (stdlib; `hook_impl.py:143-165`)', ['rc', 'cmd', 'str()'],
      [[str(e['returncode']), r(untag(e['cmd'])), code(e['out'])] for e in V['stdlib_calledprocesserror_str']])
table('V-INT: `clientlib.parse_version` = `tuple(int(p) for p in s.split("."))`', ['s', 'result'],
      [[r(e['in']), res({'ok': tuple(e['out']['ok'])} if 'ok' in e['out'] else e['out'])] for e in V['parse_version']])
table('V-SQ: `shlex.quote`', ['s', 'quote(s)'], [[r(e['in']), r(e['out'])] for e in V['shlex_quote']])
table('V-SJ: `shlex.join`', ['args', 'join(args)'], [[r(e['in']), r(e['out'])] for e in V['shlex_join']])
table('V-SS: `shlex.split` (posix, whitespace_split, no comments)', ['s', 'result'],
      [[r(e['in']), res(e['out'])] for e in V['shlex_split']])

# ---- posixpath
table('V-PN: `posixpath.normpath`', ['p', 'normpath(p)'], [[r(e['in']), r(e['out'])] for e in V['posixpath_normpath']])
table("V-PA: `posixpath.abspath` (cwd `'/home/u/repo/sub'`)", ['p', 'abspath(p)'],
      [[r(e['in']), res(e['out'])] for e in V['posixpath_abspath']])
table('V-PJ: `posixpath.join`', ['args', 'join(*args)'], [[r(e['in']), r(e['out'])] for e in V['posixpath_join']])
table("V-PR: `posixpath.relpath` (cwd `'/home/u/repo/sub'`)", ['path', 'start', 'relpath'],
      [[r(e['path']), r(e['start']), res(e['out'])] for e in V['posixpath_relpath']])
table('V-PC: `posixpath.commonpath`', ['paths', 'result'],
      [[r(e['in']), res(e['out'])] for e in V['posixpath_commonpath']])
table('V-PP2: `posixpath` split family', ['p', 'dirname', 'basename', 'splitext', 'isabs'],
      [[r(e['in']), r(e['dirname']), r(e['basename']), r(tuple(e['splitext'])), str(e['isabs'])]
       for e in V['posixpath_parts']])
pe = {}
for e in V['posixpath_expanduser']:
    pe.setdefault(json.dumps(e['env']), {})[e['in']] = e['out']['ok']
inputs = ['~', '~/', '~/.cache', 'x/~', '~~', '~/~']
table('V-PE: `posixpath.expanduser` by `HOME` value', ['HOME'] + [code(i) for i in inputs],
      [[r(json.loads(k)['HOME'])] + [r(v[i]) for i in inputs] for k, v in pe.items()])
fx = V['posixpath_realpath_fixture']
table('V-RP: `posixpath.realpath` (non-strict) on fixture: dirs `a`, `a/b`; file `a/f`; symlinks ' +
      ', '.join(f'`{n}`→`{t}`' for n, t in fx['symlinks']) + '; cwd `$ROOT`',
      ['input', 'realpath'], [[r(c['in']), r(c['out'])] for c in fx['cases']])

# ---- ntpath
table('V-NN: `ntpath.normpath`', ['p', 'normpath(p)'], [[r(e['in']), r(e['out'])] for e in V['ntpath_normpath']])
table('V-NJ: `ntpath.join`', ['args', 'join(*args)'], [[r(e['in']), r(e['out'])] for e in V['ntpath_join']])
table("V-NA: `ntpath.abspath` fallback (cwd `'C:\\\\Users\\\\u\\\\repo\\\\sub'`); `real≠` = real Windows (GetFullPathNameW) may differ",
      ['p', 'abspath(p)', 'real≠'],
      [[r(e['in']), res(e['out']), 'yes' if e['real_windows_may_differ'] else ''] for e in V['ntpath_abspath_fallback']])
table("V-NR: `ntpath.relpath` (cwd `'C:\\\\Users\\\\u\\\\repo\\\\sub'`)", ['path', 'start', 'relpath', 'real≠'],
      [[r(e['path']), r(e['start']), res(e['out']), 'yes' if e['real_windows_may_differ'] else ''] for e in V['ntpath_relpath']])
table('V-NC: `ntpath.commonpath` (3.12)', ['paths', 'result'],
      [[r(e['in']), res(e['out'])] for e in V['ntpath_commonpath']])
table('V-NP: `ntpath` split family (3.12)', ['p', 'dirname', 'basename', 'splitext', 'splitdrive', 'isabs', 'normcase'],
      [[r(e['in']), r(e['dirname']), r(e['basename']), r(tuple(e['splitext'])), r(tuple(e['splitdrive'])),
        str(e['isabs']), r(e['normcase'])] for e in V['ntpath_parts']])
ne = {}
for e in V['ntpath_expanduser']:
    ne.setdefault(json.dumps(e['env'], sort_keys=True), {})[e['in']] = e['out']['ok']
ninputs = ['~', '~/.cache', '~\\.cache', '~other\\x']
table('V-NE: `ntpath.expanduser` by environment', ['env'] + [code(i) for i in ninputs],
      [[code(k)] + [r(v[i]) for i in ninputs] for k, v in ne.items()])

# ---- composites 2
table('V-EXE: `lang_base.exe_exists` pure part (after `find_executable` → `found`, `expanduser("~")` → `home`)',
      ['flavor', 'found', 'home', 'exe_exists'],
      [[e['flavor'], r(e['found']), r(e['home']), str(e['out'])] for e in V['exe_exists_pure']])
table('V-ADJ: `_adjust_args_and_chdir` (`main.py:175-199`): abspath in cwd, then relpath from toplevel',
      ['flavor', 'cwd', 'toplevel', 'kind', 'arg', 'exists', 'result', 'Classifier (nt `\\`→`/`)'],
      [[e['flavor'], r(e['cwd']), r(e['toplevel']), e['kind'], r(e['arg']), str(e['exists']), res(e['out']),
        r(e['out'].get('classifier', '')) if e['out'].get('classifier') else ''] for e in V['adjust_args_and_chdir']])
fe = V['find_executable_posix_fixture']
table('V-FE: `parse_shebang.find_executable` (POSIX) on fixture: ' + ', '.join(f'`{f}` ({m})' for f, m in fe['files_mode']) +
      '; dir `bin3/tool/`; cwd `$ROOT`', ['exe', 'env', 'result'],
      [[r(c['exe']), code(json.dumps(c['env'])), r(c['out'])] for c in fe['cases']])

with open(sys.argv[2], 'w', encoding='utf-8', newline='\n') as f:
    f.write('\n'.join(out) + '\n')
print('lines', len(out))
