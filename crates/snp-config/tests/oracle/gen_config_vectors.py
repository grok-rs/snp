"""Generates snp-config's oracle vectors from pre-commit 4.6.2 (WP-21).

Run with the interpreter of a pre-commit 4.6.2 venv (`target/oracle/pc462-py312`, CPython 3.12.3):

    target/oracle/pc462-py312/bin/python -I crates/snp-config/tests/oracle/gen_config_vectors.py \
        crates/snp-config/tests/vectors [refs-dir]

It writes `load.json` (config / manifest validation: warnings, `str(InvalidConfigError)`, uncaught exceptions,
FutureWarnings), `merge.json` (`repository._hook` + `Hook.create`: merged fields, W11, E1-E3), `migrate.json`
(`migrate_config`: printed line and file bytes) and `revline.json` (`autoupdate._write_new_config`). Cases where
snp deliberately differs carry `divergence` and the snp expectation (`snp_*` fields); everything else is replayed
byte for byte by `tests/vectors.rs`. Inputs are fixed; no network, no git.
"""
from __future__ import annotations

import contextlib
import io
import json
import logging
import os
import re
import sys
import tempfile
import types
import warnings

import pre_commit.constants as C
from pre_commit import clientlib
from pre_commit import repository
from pre_commit.clientlib import InvalidConfigError, InvalidManifestError
from pre_commit.commands import autoupdate
from pre_commit.commands.migrate_config import migrate_config
from pre_commit.hook import Hook
from pre_commit.prefix import Prefix

assert C.VERSION == '4.6.2', C.VERSION

LOG = logging.getLogger('pre_commit')


class Capture(logging.Handler):
    def __init__(self) -> None:
        super().__init__()
        self.records: list[tuple[str, str]] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append((record.levelname, record.getMessage()))


@contextlib.contextmanager
def captured():
    h = Capture()
    LOG.addHandler(h)
    old = LOG.level
    LOG.setLevel(logging.DEBUG)
    LOG.propagate = False
    try:
        yield h
    finally:
        LOG.removeHandler(h)
        LOG.setLevel(old)


def as_input(data: bytes) -> dict:
    try:
        return {'input': data.decode('utf-8')}
    except UnicodeDecodeError:
        return {'input_hex': data.hex()}


def run_load(kind: str, data: bytes, name: str) -> dict:
    re.purge()  # FutureWarnings are emitted at compile time; a cached pattern would not warn again
    out: dict = {'warnings': [], 'error': None, 'exc': None, 'future_warnings': []}
    with tempfile.TemporaryDirectory() as d:
        cwd = os.getcwd()
        os.chdir(d)
        try:
            if data is not None:
                with open(name, 'wb') as f:
                    f.write(data)
            fn = clientlib.load_config if kind == 'config' else clientlib.load_manifest
            with captured() as h, warnings.catch_warnings(record=True) as w:
                warnings.simplefilter('always')
                try:
                    fn(name)
                except (InvalidConfigError, InvalidManifestError) as e:
                    out['error'] = str(e)
                except Exception as e:  # noqa: BLE001 - the crash text is the vector
                    out['exc'] = f'{type(e).__name__}: {e}'
            out['warnings'] = [m for lvl, m in h.records if lvl == 'WARNING']
            out['future_warnings'] = [
                str(x.message) for x in w if issubclass(x.category, FutureWarning)
            ]
        finally:
            os.chdir(cwd)
    return out


# --- load cases ---------------------------------------------------------------------------------------------------

LOCAL_HOOK = '''\
repos:
-   repo: local
    hooks:
    -   id: do_not_commit
        name: Block if "DO NOT COMMIT" is found
        entry: DO NOT COMMIT
        language: pygrep
        files: ^(.*)$
'''

V10 = r'''repos:
- repo: https://github.com/pre-commit/pre-commit-hooks
  rev: main
  hooks:
  - id: trailing-whitespace
    stages: [commit]
    files: 'foo/*'
    exclude: 'a[\\/]b'
    language: system
    foo: bar
  extra: 1
default_stages: [commit, push]
files: '/*'
exclude: '[/\\]'
bogus_top: 1
ci: {skip: [x]}
'''


def local(hook_extra: str, hook_id: str = 'a') -> str:
    return (
        'repos:\n-   repo: local\n    hooks:\n'
        f'    -   id: {hook_id}\n        name: a\n        entry: a\n        language: system\n'
        + ''.join(f'        {line}\n' for line in hook_extra.splitlines() if line)
    )


def remote(repo_extra: str = '', hook_extra: str = '', rev: str = 'v1.0.0') -> str:
    return (
        f'repos:\n-   repo: https://example.com/r\n    rev: {rev}\n'
        + ''.join(f'    {line}\n' for line in repo_extra.splitlines() if line)
        + '    hooks:\n    -   id: h\n'
        + ''.join(f'        {line}\n' for line in hook_extra.splitlines() if line)
    )


def meta(hook_extra: str, hook_id: str = 'identity') -> str:
    return (
        'repos:\n-   repo: meta\n    hooks:\n'
        f'    -   id: {hook_id}\n'
        + ''.join(f'        {line}\n' for line in hook_extra.splitlines() if line)
    )


C_CFG = 'C-cfg'
CASES: list[dict] = []


def case(name: str, text, kind: str = 'config', file: str = 'cfg.yaml', **kw) -> None:
    data = text.encode() if isinstance(text, str) else text
    CASES.append({'name': name, 'kind': kind, 'file': file, 'data': data, **kw})


# Appendix A
case('V1 repos empty', 'repos: []\n')
case('V3 not a file', None, file='nonexist.yaml')
case('V4 empty file', '', file='empty.yaml')
case('V4 comment only', '# just a comment\n')
case('V4 scalar', 'foo\n')
case('V4 list root', '-   repo: local\n    hooks: []\n')
case('V5 local missing name', 'repos:\n-   repo: local\n    hooks:\n    -   id: a\n        entry: a\n        language: system\n', file='local_missing.yaml')
case('V6 bogus language', local('').replace('system', 'bogus'), file='a1.yaml')
case('V7 bogus stage', local('stages: [commit, push, bogus]'))
case('V8 invalid files regex', local("files: '['"))
case('V9 unknown type tag', local('types: [python, nonsense]'))
case('V10 warnings only', V10)
case('V11 missing rev', 'repos:\n-   repo: https://x/y\n    hooks: []\n')
case('V11 rev on local', 'repos:\n-   repo: local\n    rev: v1\n    hooks: []\n')
case('V11 float rev on local', 'repos:\n-   repo: local\n    rev: 1.0\n    hooks: []\n')
case('V11 missing hooks', 'repos:\n-   repo: https://x/y\n    rev: v1\n')
case('V11 missing repo', 'repos:\n-   rev: v1\n    hooks: []\n')
case('V11 repos map', 'repos: {}\n')
case('V11 repos scalars', 'repos: [foo]\n')
case('V11 repo int', 'repos:\n-   repo: 123\n    rev: v1\n    hooks: []\n')
for rev in ['1.10', '2020-01-01', '0123', '~', '1234567', '0700', '""', '1e3456', '0800', '123456789012345678901234567890123']:
    case(f'V11 rev {rev}', f'repos:\n-   repo: https://x/y\n    rev: {rev}\n    hooks: []\n')
case('V11 hook id int', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: 1}]\n')
case('V11 hooks scalars', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [foo]\n')
case('V11 hooks map', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: {}\n')
case('V11 hook missing id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{name: x}]\n')
case('V12 min version 99', 'minimum_pre_commit_version: "99"\nrepos: []\n')
case('V12 min version before repos', 'minimum_pre_commit_version: "99"\nrepos: 5\n')
case('V12 min version float', 'minimum_pre_commit_version: 1.0\nrepos: []\n')
case('V12 min version ok', 'minimum_pre_commit_version: "4.6.2"\nrepos: []\n')
case('V12 min version 4.6.2.0', 'minimum_pre_commit_version: "4.6.2.0"\nrepos: []\n')
case('V12 min version unicode digit', 'minimum_pre_commit_version: "\u0664"\nrepos: []\n')
case('V12 min version spaces', 'minimum_pre_commit_version: " 4 "\nrepos: []\n')
case('V12 min version underscore', 'minimum_pre_commit_version: "4_0"\nrepos: []\n')
case('V12 install hook types legacy', 'default_install_hook_types: [pre-commit, commit]\nrepos: []\n')
case('V12 install hook types str', 'default_install_hook_types: pre-push\nrepos: []\n')
case('V12 dlv unknown', 'default_language_version: {python: python3.11, rubyx: "2"}\nrepos: []\n')
case('V12 dlv system', 'default_language_version: {python_venv: x, system: y}\nrepos: []\n')
case('V12 dlv float', 'default_language_version: {python: 3.11}\nrepos: []\n')
case('V12 dlv list', 'default_language_version: [python]\nrepos: []\n')
case('V12 default stages bogus', 'default_stages: [commit, manual, bogus]\nrepos: []\n')
case('V12 default stages str', 'default_stages: commit\nrepos: []\n')
case('V12 fail_fast yes', 'fail_fast: yes\nrepos: []\n')
case('V12 fail_fast str', 'fail_fast: "true"\nrepos: []\n')
case('V12 fail_fast int', 'fail_fast: 1\nrepos: []\n')
case('V12 exclude bad regex', 'exclude: "(?P<x"\nrepos: []\n')
case('V12 ci list', 'ci: [1]\nrepos: []\n')
case('V12 ci null', 'ci: ~\nrepos: []\n')
case('V12 default stages empty', 'default_stages: []\nrepos: []\n')
case('V13 meta bogus id', meta('', 'bogus'))
case('V13 meta entry', meta('entry: echo'))
case('V13 meta language system', meta('language: system'))
case('V13 meta warnings', meta('files: "x/*"\nstages: [commit]\nfoo: 1', 'check-hooks-apply'))
case('V14 manifest valid', '- {id: a, name: a, entry: a, language: system}\n', kind='manifest')
case('V14 manifest map', 'hooks:\n  - id: a\n', kind='manifest', file='h2.yaml')
case('V14 manifest missing language', '- {id: a, name: a, entry: a}\n', kind='manifest', file='h3.yaml')
case('V14 manifest min version', '- {id: a, name: a, entry: a, language: system, minimum_pre_commit_version: "99.0"}\n', kind='manifest')
case('V14 manifest empty list', '[]\n', kind='manifest')
case('V14 manifest empty', '', kind='manifest', file='h6.yaml')
case('V14 manifest no warnings', '- {id: a, name: a, entry: a, language: system, foo: 1, stages: [commit], files: "a/*"}\n', kind='manifest')
case('V14 manifest duplicate ids', '- {id: a, name: a, entry: a, language: system}\n- {id: a, name: b, entry: b, language: system}\n', kind='manifest')
case('V15 flow node', 'repos: [\n')
case('V15 tab', 'repos: []\n\tfoo: 1\n')
case('V15 two documents', 'repos: []\n---\nrepos: []\n')
case('V15 value tag', local('args: [=]'))
case('V15 bad month', 'repos: []\nfoo: 2020-13-45\n')
case('V15 utf16', '\ufeffrepos: []\n'.encode('utf-16'))
case('V15 bad continuation', b'# comment \xe9x\nrepos: []\n')
case('V15 bom', '\ufeffrepos: []\n')
case('V15 duplicate key', 'repos: []\nrepos: [1]\n')
case('V15 binary repos', 'repos: !!binary aGVsbG8=\n')
case('V15 set files', 'files: !!set {a}\nrepos: []\n')
case('V15 anchors and merge', 'x: &x {repo: local, hooks: []}\nrepos: [*x, {<<: *x, hooks: [{id: a, name: a, entry: a, language: system, <<: {files: "^b"}}]}]\n')
case('V15 yaml 1.2 directive', '%YAML 1.2\n---\nrepos: []\nfail_fast: on\n')
case('V15 crlf', 'repos:\r\n-   repo: local\r\n    hooks:\r\n    -   id: a\r\n        name: a\r\n        entry: a\r\n        language: bogus\r\n')
case('V15 nan id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: .nan}]\n')
case('V15 date id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: 2020-01-01}]\n')
case('V15 list id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: [1, "x"]}]\n')
case('V15 map id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: {a: 1}}]\n')
case('V15 bytes id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: !!binary aGk=}]\n')
case('V15 quote id', 'repos:\n-   repo: https://x/y\n    rev: v1\n    hooks: [{id: "it\'s"}]\n')
case('V15 language yes', local('').replace('language: system', 'language: yes'))
case('V15 language float', local('').replace('language: system', 'language: 1.5'))

# T10-CFG rows (10 §5.1)
case('T10-CFG-01 bad tags', local('types: [definitely-not-a-tag]\ntypes_or: [fiel]'))
case('T10-CFG-01 file tag', local('types: [file]'))
case('T10-CFG-02 valid', "repos:\n-   repo: git@github.com:pre-commit/pre-commit-hooks\n    rev: cd74dc150c142c3be70b24eaf0b02cae9d235f37\n    hooks:\n    -   id: pyflakes\n        files: '\\.py$'\n        args: [foo, bar, baz]\n")
case('T10-CFG-03 exclude 0', "repos:\n-   repo: git@github.com:pre-commit/pre-commit-hooks\n    rev: cd74dc150c142c3be70b24eaf0b02cae9d235f37\n    hooks:\n    -   id: pyflakes\n        files: '\\.py$'\n        exclude: 0\n        args: [foo, bar, baz]\n")
case('T10-CFG-04 local rev', LOCAL_HOOK.replace('repo: local\n', 'repo: local\n    rev: foo\n'))
case('T10-CFG-05 sample local', LOCAL_HOOK)
case('T10-CFG-07 ci and meta', 'ci: {skip: [foo]}\nrepos: [{repo: meta, hooks: [{id: identity}]}]\n')
case('T10-CFG-07 ci string', 'ci: invalid\nrepos: []\n')
for rev in ['v0.12.4', 'b27f281', 'b27f281eb9398fc8504415d7fbdabf119ea8c5e1', '19.10b0', '4.3.21-2', "''", 'HEAD', 'stable', 'master', 'some_branch_name', 'deadbeef', 'abc', 'v1', 'main']:
    case(f'T10-CFG-08 rev {rev}', f'repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: {rev}\n    hooks: [{{id: flake8}}]\n')
case('T10-CFG-09 meta rev', 'repos:\n-   repo: meta\n    rev: 3.7.7\n    hooks: [{id: identity}]\n')
case('T10-CFG-10 files none', local('files: ~'))
case('T10-CFG-10 files empty', 'files:\nrepos: []\n')
for pat in [r'dir/*.py', r'dir[\/].*\.py', r'dir[/\\].*\.py', r'dir[\\/].*\.py', r'[\\/][/\\]x/*']:
    q = "'" + pat + "'"
    case(f'T10-CFG-11 remote {pat}', f'repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: v1.0.0\n    hooks:\n    -   id: flake8\n        files: {q}\n        exclude: {q}\n')
    case(f'T10-CFG-11 local {pat}', LOCAL_HOOK.replace("files: ^(.*)$", f'files: {q}'))
    case(f'T10-CFG-11 meta {pat}', meta(f'files: {q}'))
    case(f'T10-CFG-12 top {pat}', f'files: {q}\nexclude: {q}\nrepos: []\n')
case('T10-CFG-13 invalid stage', LOCAL_HOOK + '        stages: [invalid]\n')
case('T10-CFG-14 hook legacy stages', LOCAL_HOOK + '        stages: [commit, push]\n')
case('T10-CFG-14 top legacy stages', 'default_stages: [commit, push]\nrepos: []\n')
case('T10-CFG-14 modern stages', 'default_stages: [pre-commit, pre-push]\nrepos: []\n')
case('T10-CFG-15 system and script', LOCAL_HOOK.replace('pygrep', 'system') + LOCAL_HOOK.replace('repos:\n', '').replace('pygrep', 'script'))
case('T10-CFG-15 missing language', LOCAL_HOOK.replace('        language: pygrep\n', ''))
case('T10-CFG-16 manifest python', "- {id: a, name: b, entry: c, language: python, files: '\\.py$'}\n", kind='manifest')
case('T10-CFG-16 manifest version', "- {id: a, name: b, entry: c, language: python, language_version: python3.4, files: '\\.py$'}\n", kind='manifest')
case('T10-CFG-16 manifest always run', "- {id: a, name: b, entry: c, language: python, files: '', always_run: true}\n", kind='manifest')
case('T10-CFG-17 meta missing id', meta('', 'i-dont-exist'))
case('T10-CFG-17 meta python', meta('language: python'))
case('T10-CFG-17 meta name false', meta('name: False'))
case('T10-CFG-17 meta entry', meta('entry: echo hi'))
case('T10-CFG-18 meta defaults', meta('', 'check-hooks-apply'))
case('T10-CFG-19 dlv pony', "default_language_version: {pony: '1.0'}\nrepos: []\n")
case('T10-CFG-19 dlv int', 'default_language_version: {python: 3}\nrepos: []\n')
case('T10-CFG-21 top', "minimum_pre_commit_version: '999'\nrepos: []\n")
case('T10-CFG-21 hook', LOCAL_HOOK + "        minimum_pre_commit_version: '999'\n")
case('T10-CFG-21 repos 5', "minimum_pre_commit_version: '999'\nrepos: 5\n")
case('T10-CFG-21 zero', "minimum_pre_commit_version: '0'\nrepos: []\n")
case('T10-CFG-22 root', 'repos: []\nfoo: 1\n')
case('T10-CFG-22 repo', 'repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: 3.7.7\n    hooks: [{id: flake8}]\n    args: [foo]\n')
case('T10-CFG-23 default stages', 'default_stages: [commit-msg, push, commit, merge-commit]\nrepos: []\n')
case('T10-CFG-24 manifest hooks map', 'hooks: {}', kind='manifest', file='.pre-commit-hooks.yaml')
case('T10-CFG-25 yaml bools', local('always_run: yes\npass_filenames: off'))
case('T10-CFG-26 rev float', 'repos:\n-   repo: https://example.com/r\n    rev: 1.10\n    hooks: [{id: h}]\n')
case('T10-CFG-27 duplicate repos', 'repos: []\nrepos: []\n')
case('T10-CFG-28 anchors', 'x-common: &common\n    language: system\n    entry: echo\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        <<: *common\n')
case('T10-CFG-29 missing name', 'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        entry: x\n        language: system\n')

# X13-CFG rows (13 X.4.1): C-cfg conversions.
TOP_MIN = '\n==> File cfg.yaml\n==> At Config()\n==> At key: minimum_pre_commit_version\n'
for lit in ['abc', '', '4.', '4.6.3rc1', '4..6', '0x10', '1.x']:
    case(f'X13-CFG-01 min version {lit!r}', f'minimum_pre_commit_version: "{lit}"\nrepos: []\n',
         divergence='D-CRASH-01', snp_ctx=TOP_MIN)
case('X13-CFG-01 hook min version', local("minimum_pre_commit_version: abc"), divergence='D-CRASH-01',
     snp_ctx="\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='a')\n==> At key: minimum_pre_commit_version\n")
case('X13-CFG-02 int root key', 'repos: []\n1: x\n', divergence='D-CRASH-02',
     snp_warnings=['Unexpected key(s) present at root: 1'])
case('X13-CFG-03 mixed root keys', 'repos: []\n1: x\nfoo: y\n', divergence='D-CRASH-02',
     snp_warnings=['Unexpected key(s) present at root: 1, foo'])
case('X13-CFG-03 bool and none keys', 'repos: []\ntrue: x\n~: y\nbar: z\n', divergence='D-CRASH-02',
     snp_warnings=['Unexpected key(s) present at root: None, True, bar'])
case('X13-CFG-04 stages list', local('stages: [[a]]'), divergence='D-CRASH-03',
     snp_error="\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='a')\n==> At key: stages\n==> At index 0\n=====> Expected one of commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: ['a']")
case('X13-CFG-04 language list', local('').replace('language: system', 'language: [x]'), divergence='D-CRASH-03',
     snp_error="\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='a')\n==> At key: language\n=====> Expected one of conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script but got: ['x']")
case('X13-CFG-04 types list', local('types: [[a]]'), divergence='D-CRASH-03',
     snp_error="\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='a')\n==> At key: types\n==> At index 0\n=====> Type tag ['a'] is not recognized.  Try upgrading identify and pre-commit?")
for pat in ['a{4294967295}', 'a{4294967296}', 'a{0,4294967295}', 'a{99999999999,}']:
    case(f'X13-CFG-05 {pat}', f"files: '{pat}'\nrepos: []\n", divergence='D-CRASH-04',
         snp_error=f"\n==> File cfg.yaml\n==> At Config()\n==> At key: files\n=====> '{pat}' is not a valid python regex")
case('X13-CFG-05 a{4294967294}', "files: 'a{4294967294}'\nrepos: []\n")
case('X13-CFG-06 nesting 485', "files: '" + '(' * 485 + ')' * 485 + "'\nrepos: []\n", divergence='D-CRASH-04',
     snp_error="\n==> File cfg.yaml\n==> At Config()\n==> At key: files\n=====> '" + '(' * 485 + ')' * 485 + "' is not a valid python regex")
case('X13-CFG-06 nesting 484', "files: '" + '(?:' * 484 + ')' * 484 + "'\nrepos: []\n")
for pat in ['(?a)(?u)x', '(?u)(?a)x']:
    case(f'X13-CFG-07 {pat}', f"exclude: '{pat}'\nrepos: []\n", divergence='D-CRASH-04',
         snp_error=f"\n==> File cfg.yaml\n==> At Config()\n==> At key: exclude\n=====> '{pat}' is not a valid python regex")
case('X13-CFG-07 (?au)x', "exclude: '(?au)x'\nrepos: []\n")
case('X13-CFG-07 (?u)(?a:x)', "exclude: '(?u)(?a:x)'\nrepos: []\n")
for pat in ['[[]', '[[:alpha:]]', '[a&&b]', '[a||b]', '[a~~b]']:
    case(f'X13-SM-03 {pat}', f"files: '{pat}'\nrepos: []\n")
# `[a--b]` is an invalid range: CPython warns while parsing and then raises `re.error`; snp reports only the
# validation error (snp-pyre returns no warnings for a pattern that fails to parse; D-MISC-04).
case('X13-SM-03 [a--b]', "files: '[a--b]'\nrepos: []\n", divergence='D-MISC-04', snp_future_warnings=[])
case('X13-SM-03 hook and top dedupe', "files: '[[]'\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        files: '[[]'\n        exclude: '[a&&b]'\n")
case('X13-SM-03 hook before top', "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        files: '[[a]'\nfiles: 'a[b&&c]'\n")
case('X13-SM-03 top first in the file', "files: 'a[b&&c]'\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        exclude: '[[a]'\n")
case('X13-SM-03 before a later error', "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        files: '[[a]'\nfail_fast: 3\n")
case('X13-SM-03 same hook later error', "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        files: '[a||b]'\n        types: [nonsense]\n")
case('X13-SM-03 manifest before error', "- {id: a, name: a, entry: a, language: system, exclude: '[a~~b]'}\n- {id: b, name: b, entry: b, language: bogus}\n", kind='manifest')
case('X13-CFG-13 manifest crash rows', "- {id: a, name: a, entry: a, language: [x]}\n", kind='manifest', divergence='D-CRASH-03',
     snp_error="\n==> File cfg.yaml\n==> At Hook(id='a')\n==> At key: language\n=====> Expected one of conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script but got: ['x']")
case('X13-CFG-13 manifest overflow', "- {id: a, name: a, entry: a, language: system, files: 'a{4294967296}'}\n", kind='manifest', divergence='D-CRASH-04',
     snp_error="\n==> File cfg.yaml\n==> At Hook(id='a')\n==> At key: files\n=====> 'a{4294967296}' is not a valid python regex")
case('X13-CFG-13 manifest min version', "- {id: a, name: a, entry: a, language: system, minimum_pre_commit_version: x}\n", kind='manifest', divergence='D-CRASH-01',
     snp_ctx="\n==> File cfg.yaml\n==> At Hook(id='a')\n==> At key: minimum_pre_commit_version\n")

# Extension keys under pre-commit (they are unknown keys there; snp in Strict mode must agree).
case('EXT root keys', 'repos: []\npriorities: {early: 1}\noutput: summary\nx-anchor: 1\n', ext=True)
case('EXT repo key', 'repos:\n-   repo: local\n    x-group: 1\n    hooks: []\n', ext=True)
case('EXT hook keys', local('priority: 1\nenv: {A: b}\nstage_fixed: true\nx-snp: 1'), ext=True)
case('EXT builtin', 'repos:\n-   repo: builtin\n    hooks: [{id: trailing-whitespace}]\n', ext=True)

# Corpus: real configs and manifests.
REFS = sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser('~/.cache/snp-dev/refs')
for proj in ['pre-commit', 'pre-commit-hooks', 'cfgv', 'identify', 'pygrep-hooks']:
    for fname, kind in [('.pre-commit-config.yaml', 'config'), ('.pre-commit-hooks.yaml', 'manifest')]:
        p = os.path.join(REFS, proj, fname)
        if os.path.exists(p):
            with open(p, 'rb') as f:
                case(f'corpus {proj}/{fname}', f.read(), kind=kind)


def gen_load() -> list[dict]:
    out = []
    for c in CASES:
        res = run_load(c['kind'], c['data'], c['file'])
        entry = {'name': c['name'], 'kind': c['kind'], 'file': c['file']}
        if c['data'] is None:
            entry['missing'] = True
        else:
            entry.update(as_input(c['data']))
        entry.update(res)
        for k in ('divergence', 'snp_error', 'snp_warnings', 'ext'):
            if k in c:
                entry[k] = c[k]
        if 'snp_future_warnings' in c:
            entry['snp_future_warnings'] = c['snp_future_warnings']
            entry.setdefault('snp_error', res['error'])
        if 'snp_ctx' in c:
            assert res['exc'] and res['exc'].startswith('ValueError: '), (c['name'], res)
            entry['snp_error'] = c['snp_ctx'] + '=====> ' + res['exc'][len('ValueError: '):]
        out.append(entry)
    return out


# --- merge cases --------------------------------------------------------------------------------------------------

class _Lang:
    def __init__(self, mod):
        self.ENVIRONMENT_DIR = mod.ENVIRONMENT_DIR

    def get_default_version(self):  # resolved by snp-repo with probes, never by snp-config
        return C.DEFAULT


MERGE_CASES = [
    {
        'name': 'R1 local unknown keys',
        'config': 'repos:\n-   repo: local\n    hooks:\n    -   id: echo1\n        name: echo1\n        entry: echo\n        language: system\n        zzz: 1\n        foo: 2\n',
    },
    {
        'name': 'R3 language_version on system',
        'config': 'repos:\n-   repo: local\n    hooks:\n    -   id: lv\n        name: lv\n        entry: echo\n        language: system\n        language_version: python3\n',
    },
    {
        'name': 'R3 additional_dependencies on script',
        'config': 'repos:\n-   repo: local\n    hooks:\n    -   id: ad\n        name: ad\n        entry: echo\n        language: script\n        additional_dependencies: [x]\n',
    },
    {
        'name': 'R4 default_language_version on unsupported',
        'config': 'default_language_version: {unsupported: x}\nrepos:\n-   repo: local\n    hooks:\n    -   id: s\n        name: s\n        entry: echo\n        language: system\n',
    },
    {
        'name': 'R5 empty default stages',
        'config': 'default_stages: []\nrepos:\n-   repo: local\n    hooks:\n    -   id: s\n        name: s\n        entry: echo\n        language: system\n',
    },
    {
        'name': 'R6 manifest and config unknown keys',
        'config': 'repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: hello\n        bar: 1\n',
        'manifest': '- {id: hello, name: hello, entry: echo, language: system, foo: 1, stages: [commit, push, manual, commit]}\n',
    },
    {
        'name': 'R7 whole-key override',
        'config': 'repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: h\n        stages: []\n        args: [--x]\n        language: system\n        name: renamed\n',
        'manifest': '- {id: h, name: h, entry: echo, language: python, stages: [merge-commit], args: [--y, --z], types: [python]}\n',
    },
    {
        'name': 'language_version on an env-less override',
        'config': 'repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: h\n        language: system\n',
        'manifest': '- {id: h, name: h, entry: echo, language: python, language_version: python3.11}\n',
    },
    {
        'name': 'every key overridden',
        'config': "repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: h\n        name: n2\n        entry: e2\n        alias: al\n        files: '^a'\n        exclude: '^b'\n        types: [text]\n        types_or: [python, rust]\n        exclude_types: [binary]\n        additional_dependencies: [dep==1]\n        args: [--x]\n        always_run: true\n        fail_fast: true\n        pass_filenames: false\n        description: d2\n        language_version: python3.12\n        log_file: out.log\n        minimum_pre_commit_version: '1.0'\n        require_serial: true\n        stages: [commit, manual]\n        verbose: true\n",
        'manifest': '- {id: h, name: h, entry: echo, language: python}\n',
    },
    {
        'name': 'language override changes the default_language_version lookup',
        'config': 'default_language_version: {node: "18.0.0", python: python3.12}\nrepos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: h\n        language: node\n',
        'manifest': '- {id: h, name: h, entry: echo, language: python}\n',
    },
    {
        'name': 'meta defaults and stages',
        'config': 'default_stages: [pre-push]\nrepos:\n-   repo: meta\n    hooks:\n    -   id: identity\n        args: [--foo]\n    -   id: check-useless-excludes\n        stages: [manual]\n',
    },
    {
        'name': 'E1 unknown id',
        'config': 'repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: nonexistent\n',
        'manifest': '- {id: hello, name: hello, entry: echo, language: system}\n',
    },
    {
        'name': 'remote repo: every _hook before any Hook.create',
        'config': 'default_language_version: {pygrep: p}\nrepos:\n-   repo: https://x/r\n    rev: v1\n    hooks:\n    -   id: a\n        zz: 1\n    -   id: b\n    -   id: c\n',
        'manifest': '- {id: a, name: a, entry: a, language: system, x-a: 3}\n- {id: b, name: b, entry: b, language: system, x-a: 3}\n- {id: c, name: c, entry: c, language: pygrep}\n',
    },
    {
        'name': 'remote repo: W11 after every hook merged',
        'config': 'repos:\n-   repo: https://x/r\n    rev: v1\n    hooks:\n    -   id: a\n        zz: 1\n    -   id: b\n',
        'manifest': '- {id: a, name: a, entry: a, language: system}\n- {id: b, name: b, entry: b, language: system, x-a: 3}\n',
    },
    {
        'name': 'local repo: Hook.create per hook',
        'config': 'repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        zz: 1\n    -   id: b\n        name: b\n        entry: b\n        language: system\n        language_version: x\n',
    },
    {
        'name': 'duplicate manifest ids: last wins',
        'config': 'repos:\n-   repo: https://example.com/r\n    rev: v1\n    hooks:\n    -   id: a\n',
        'manifest': '- {id: a, name: first, entry: one, language: system}\n- {id: a, name: second, entry: two, language: system}\n',
    },
]


def hook_view(h: dict) -> dict:
    return {k: h[k] for k in sorted(h) if k in repository_keys}


repository_keys = set(Hook._fields) - {'src', 'prefix'}


def gen_merge() -> list[dict]:
    langs = {k: _Lang(v) for k, v in repository.languages.items()}
    out = []
    for mc in MERGE_CASES:
        with tempfile.TemporaryDirectory() as d:
            cfg_path = os.path.join(d, 'cfg.yaml')
            with open(cfg_path, 'w') as f:
                f.write(mc['config'])
            root = clientlib.load_config(cfg_path)
            manifest = None
            if 'manifest' in mc:
                mp = os.path.join(d, 'm.yaml')
                with open(mp, 'w') as f:
                    f.write(mc['manifest'])
                manifest = clientlib.load_manifest(mp)
        result = {'name': mc['name'], 'config': mc['config'], 'manifest': mc.get('manifest'),
                  'hooks': [], 'logs': [], 'exit': None}
        old = repository.languages
        repository.languages = langs
        try:
            with captured() as h:
                try:
                    for repo in root['repos']:
                        src = repo['repo']
                        if src in ('local', 'meta'):
                            # `_non_cloned_repository_hooks`: `_hook` and `Hook.create` per hook, interleaved.
                            for hook in repo['hooks']:
                                hk = Hook.create(src, Prefix('.'), repository._hook(hook, root_config=root))
                                result['hooks'].append({k: getattr(hk, k) for k in sorted(repository_keys)})
                            continue
                        else:
                            by_id = {hk['id']: hk for hk in manifest}
                            for hook in repo['hooks']:
                                if hook['id'] not in by_id:
                                    logging.getLogger('pre_commit').error(
                                        f'`{hook["id"]}` is not present in repository {src}.  '
                                        f'Typo? Perhaps it is introduced in a newer version?  '
                                        f'Often `pre-commit autoupdate` fixes this.',
                                    )
                                    raise SystemExit(1)
                            dcts = [repository._hook(by_id[hook['id']], hook, root_config=root) for hook in repo['hooks']]
                        for dct in dcts:
                            hk = Hook.create(src, Prefix('.'), dct)
                            result['hooks'].append({k: getattr(hk, k) for k in sorted(repository_keys)})
                except SystemExit as e:
                    result['exit'] = e.code
            result['logs'] = [[lvl, m] for lvl, m in h.records]
        finally:
            repository.languages = old
        out.append(result)
    return out


# --- migrate cases ------------------------------------------------------------------------------------------------

MIGRATE_CASES = [
    ('T10-MIG-01 list root', '-   repo: local\n    hooks:\n    -   id: foo\n        name: foo\n        entry: ./bin/foo.sh\n        language: script\n'),
    ('T10-MIG-02 header', '# comment\n\n---\n-   repo: local\n    hooks:\n    -   id: foo\n        name: foo\n        entry: ./bin/foo.sh\n        language: script\n'),
    ('T10-MIG-03 flow list', '[{\n    repo: local,\n    hooks: [{\n        id: foo, name: foo, entry: ./bin/foo.sh,\n        language: script,\n    }]\n}]'),
    ('T10-MIG-04 already migrated', 'repos:\n-   repo: local\n    hooks:\n    -   id: foo\n        name: foo\n        entry: ./bin/foo.sh\n        language: script\n'),
    ('T10-MIG-05 sha', 'repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    sha: v1.2.0\n    hooks: []\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    sha: v1.2.0\n    hooks: []\n'),
    ('T10-MIG-05 sha json', '{"repos": [{\n    "repo": "https://github.com/pre-commit/pre-commit-hooks",\n    "sha": "v1.2.0",\n    "hooks": []\n}]}\n'),
    ('T10-MIG-06 python_venv', 'repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: python_venv\n    -   id: example\n        name: example\n        entry: example\n        language: system\n'),
    ('T10-MIG-06 quoted python_venv', 'repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: "python_venv"\n'),
    ('T10-MIG-07 default stages', 'default_stages: [commit, push, merge-commit, commit-msg]\nrepos: []\n'),
    ('T10-MIG-07 hook stages', 'repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: system\n        stages: ["commit", "push", "merge-commit", "commit-msg"]\n'),
    ('T10-MIG-08 invalid yaml', '['),
    ('MC1 header list root', "# comment header\n---\n\n-   repo: https://example.com/r\n    sha: v1.0.0\n    hooks:\n    -   id: x\n        stages: [commit, \"push\", 'merge-commit', manual]\n        language: python_venv\n"),
    ('MC2 flow list root', "[{repo: local, hooks: [{id: x, name: x, entry: x, language: \"python_venv\", stages: ['commit']}]}]\n"),
    ('MC3 invalid yaml', 'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        args: [\n'),
    ('MC4 alias stages', 'default_stages: &s [commit]\nrepos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        stages: *s\n'),
    ('MC4 folded python_venv', 'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: >-\n            python_venv\n        files: a\n'),
    ('CRLF migration', '-   repo: local\r\n    hooks:\r\n    -   id: x\r\n        name: x\r\n        entry: x\r\n        language: system\r\n        stages: [commit]\r\n'),
    ('CRLF already migrated', 'repos:\r\n-   repo: local\r\n    hooks: []\r\n'),
    ('X13-CFG-12 fallback invalid', '%YAML 1.1\n---\n-   repo: local\n    hooks: []\n'),
    ('empty file', ''),
    ('scalar root', 'foo\n'),
    ('quoted sha key', "repos:\n-   repo: https://example.com/r\n    'sha': v1\n    hooks: []\n"),
    ('tagged stages', 'default_stages: [!!str commit]\nrepos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        stages:\n        - !!str push\n'),
    ('tagged sha key', 'repos:\n-   repo: https://x/r\n    !!str sha: v1\n    hooks: []\n'),
    ('anchored sha key', 'repos:\n-   repo: https://x/r\n    &k sha: v1\n    hooks: []\nx: *k\n'),
    ('system is not migrated', 'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n'),
]

MIGRATE_SNP = {
    # D-DATA-14: one rewrite per aliased node.
    'MC4 alias stages': ('D-DATA-14', 'default_stages: &s [pre-commit]\nrepos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        stages: *s\n'),
    # D-DATA-14: a rewritten node keeps its anchor (pre-commit drops it; the later alias is then undefined).
    'anchored sha key': ('D-DATA-14', 'repos:\n-   repo: https://x/r\n    &k rev: v1\n    hooks: []\nx: *k\n'),
    # D-DATA-15: a block scalar becomes a plain scalar.
    'MC4 folded python_venv': ('D-DATA-15', 'repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: python\n        files: a\n'),
    # D-DATA-09: each line keeps its own ending.
    'CRLF migration': ('D-DATA-09', 'repos:\r\n-   repo: local\r\n    hooks:\r\n    -   id: x\r\n        name: x\r\n        entry: x\r\n        language: system\r\n        stages: [pre-commit]\r\n'),
}


def gen_migrate() -> list[dict]:
    out = []
    for name, text in MIGRATE_CASES:
        entry = {'name': name, 'input': text, 'stdout': None, 'output': None, 'error': None, 'exc': None}
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, C.CONFIG_FILE)
            with open(p, 'w', newline='') as f:
                f.write(text)
            cwd = os.getcwd()
            os.chdir(d)
            try:
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    try:
                        migrate_config(C.CONFIG_FILE)
                    except InvalidConfigError as e:
                        entry['error'] = str(e)
                    except Exception as e:  # noqa: BLE001
                        entry['exc'] = f'{type(e).__name__}: {e}'
                entry['stdout'] = buf.getvalue()
                with open(C.CONFIG_FILE, newline='') as f:
                    entry['output'] = f.read()
            finally:
                os.chdir(cwd)
        if name in MIGRATE_SNP:
            entry['divergence'], entry['snp_output'] = MIGRATE_SNP[name]
        if name == 'X13-CFG-12 fallback invalid':
            entry['divergence'] = 'D-CRASH-07'
        out.append(entry)
    return out


# --- revline cases ------------------------------------------------------------------------------------------------

REVLINE_CASES = [
    # (name, text, [(config_repo_index, rev, frozen)])
    ('AUP-05 one of two stale', 'repos:\n-   repo: a\n    rev: v1\n    hooks:\n    -   id: foo\n-   repo: b\n    rev: v1\n    hooks:\n    -   id: foo\n', [(1, 'v2', None)]),
    ('AUP-07 comments kept', "repos:\n-   repo: a\n    rev: v1  # definitely the version I want!\n    hooks:\n    -   id: foo\n        # These args are because reasons!\n        args: [foo, bar, baz]\n", [(0, 'v2', None)]),
    ('AUP-08 mixed endings', 'repos:\n-   repo: a\r\n    rev: v1\r\n    hooks:\n    -   id: foo\r\n', [(0, 'v2', None)]),
    ('AUP-09 flow style', 'repos: [{repo: a, rev: v1, hooks: [# A comment!\n    {id: foo}]}]\n', [(0, 'v2', None)]),
    ('AUP-10 freeze', 'repos:\n-   repo: a\n    rev: v1.2.3\n    hooks: [{id: foo}]\n', [(0, 'a' * 40, 'v1.2.3')]),
    ('AUP-10 unfreeze', 'repos:\n-   repo: a\n    rev: ' + 'a' * 40 + '  # frozen: v1.2.3\n    hooks: [{id: foo}]\n', [(0, 'v1.2.3', None)]),
    ('AUP-14 local first', 'repos:\n-   repo: local\n    hooks: []\n-   repo: b\n    rev: v1\n    hooks: []\n', [(0, 'v2', None)]),
    ('AUP-17 double quoted', 'repos:\n-   repo: a\n    rev: "v1"\n    hooks: []\n', [(0, 'v2', None)]),
    ('AUP-17 single quoted', "repos:\n-   repo: a\n    rev: 'v1'\n    hooks: []\n", [(0, 'v2', None)]),
    ('quoting of a retyping rev', 'repos:\n-   repo: a\n    rev: v1\n    hooks: []\n', [(0, '1.0', None)]),
    ('quoting of yes', 'repos:\n-   repo: a\n    rev: v1\n    hooks: []\n', [(0, 'yes', None)]),
    ('rev at column 0', 'repos:\n- repo: a\n  rev: v1\n  hooks: []\n', [(0, 'v2', None)]),
    ('list rev line in local repo forces reformat', 'repos:\n-   repo: local\n    rev: x\n    hooks: []\n-   repo: b\n    rev: v1\n    hooks: []\n', None),
    ('D-DATA-10 freeze crlf', 'repos:\r\n-   repo: a\r\n    rev: v1  # note\r\n    hooks: []\r\n', [(0, 'b' * 40, 'v2')]),
    ('D-DATA-10 unfreeze crlf', 'repos:\r\n-   repo: a\r\n    rev: ' + 'b' * 40 + '  # frozen: v1\r\n    hooks: []\r\n', [(0, 'v2', None)]),
    ('D-DATA-16 no final newline', '# keep me\nrepos:\n-   repo: a\n    hooks: []\n    rev: v1  # c', [(0, 'v2', None)]),
    ('nothing to update', 'repos:\n-   repo: a\n    rev: v1\n    hooks: []\n', [(0, None, None)]),
]

REVLINE_SNP = {
    'D-DATA-10 freeze crlf': ('D-DATA-10', 'repos:\r\n-   repo: a\r\n    rev: ' + 'b' * 40 + '  # frozen: v2\r\n    hooks: []\r\n'),
    'D-DATA-10 unfreeze crlf': ('D-DATA-10', 'repos:\r\n-   repo: a\r\n    rev: v2\r\n    hooks: []\r\n'),
    'D-DATA-16 no final newline': ('D-DATA-16', '# keep me\nrepos:\n-   repo: a\n    hooks: []\n    rev: v2  # c'),
}


def gen_revline() -> list[dict]:
    out = []
    for name, text, updates in REVLINE_CASES:
        if updates is None:
            continue
        entry = {'name': name, 'input': text, 'updates': [], 'output': None, 'exc': None, 'reformatted': False}
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, 'cfg.yaml')
            with open(p, 'w', newline='') as f:
                f.write(text)
            with captured():
                loaded = clientlib.load_config(p)
            config_repos = [r for r in loaded['repos'] if r['repo'] not in {'local', 'meta'}]
            infos = [None] * len(config_repos)
            for i, rev, frozen in updates:
                if rev is not None:
                    infos[i] = autoupdate.RevInfo(config_repos[i]['repo'], rev, frozen)
            all_repos = loaded['repos']
            remote_idx = [k for k, r in enumerate(all_repos) if r['repo'] not in {'local', 'meta'}]
            for i, rev, frozen in updates:
                if rev is not None:
                    entry['updates'].append({'repo_index': remote_idx[i], 'rev': rev, 'frozen': frozen})
            lines = text.splitlines(True)
            idxs = [i for i, line in enumerate(lines) if autoupdate.REV_LINE_RE.match(line)]
            entry['reformatted'] = len(idxs) != len(config_repos)
            try:
                autoupdate._write_new_config(p, infos)
                with open(p, newline='') as f:
                    entry['output'] = f.read()
            except Exception as e:  # noqa: BLE001
                entry['exc'] = f'{type(e).__name__}: {e}'
        if name in REVLINE_SNP:
            entry['divergence'], entry['snp_output'] = REVLINE_SNP[name]
            entry['snp_reformatted'] = False
        out.append(entry)
    return out


def main() -> None:
    dest = sys.argv[1]
    os.makedirs(dest, exist_ok=True)
    for fname, data in [
        ('load.json', gen_load()),
        ('merge.json', gen_merge()),
        ('migrate.json', gen_migrate()),
        ('revline.json', gen_revline()),
    ]:
        with open(os.path.join(dest, fname), 'w') as f:
            json.dump(
                {'generator': 'crates/snp-config/tests/oracle/gen_config_vectors.py',
                 'pre_commit': C.VERSION, 'python': sys.version.split()[0], 'cases': data},
                f, indent=1, ensure_ascii=False,
            )
            f.write('\n')
        print(f'{fname}: {len(data)} cases')


if __name__ == '__main__':
    main()
