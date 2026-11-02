"""Oracle driver of `cargo xtask cli-goldens` (WP-12, ADR-058, 12 §A.16).

Run with an oracle interpreter (`target/oracle/pc470-py313/bin/python -I`). Two modes:

    argparse_from_json.py json SPEC.json -- ARGV...
        Build a real CPython argparse parser from `snp_argparse::export_json(spec, prog)` and handle ARGV the way
        `pre_commit.main.main` does: `[]` means `['run']`, and `help [cmd]` re-parses `[cmd, '--help']`.

    argparse_from_json.py json-plain SPEC.json -- ARGV...
        Build the parser the same way and call `parse_args(ARGV)` only (the generic zoo specs of snp-argparse).

    argparse_from_json.py textwrap CASES.json --
        Print `[textwrap.wrap(text, width, initial_indent=…, subsequent_indent=…), …]` as one JSON line for the
        `[{"text", "width", "initial", "subsequent"}, …]` cases (snp-argparse's textwrap vectors).

    argparse_from_json.py pre-commit [--prog PROG] -- ARGV...
        Run `pre_commit.main.main(ARGV)`, optionally with argparse's `prog='pre-commit'` patched to PROG (the
        `pre-commit-snp-prog` launcher), and `pre_commit.color.use_color` returning the validated string instead
        of a terminal-dependent bool.

In both modes help, version and usage errors behave exactly as argparse does (stdout/stderr, exit 0/2). When the
top-level parse succeeds for any command but `help`, the namespace is written to stdout as `\\0ns\\0` + one JSON
line (`vars(namespace)`, sorted keys; a `str` that is not valid UTF-8 becomes `{"hex": <os.fsencode(s).hex()>}`)
and the process exits 0 before any command runs.
"""
from __future__ import annotations

import argparse
import json
import os
import sys

NS_MARKER = '\0ns\0'
COLOR_CHOICES = ('auto', 'always', 'never')
_STAGES = {'commit': 'pre-commit', 'merge-commit': 'pre-merge-commit', 'push': 'pre-push'}


def use_color(setting: str) -> str:
    if setting not in COLOR_CHOICES:
        raise ValueError(setting)
    return setting


def transform_stage(stage: str) -> str:
    return _STAGES.get(stage, stage)


def _encode(v):
    if isinstance(v, str):
        try:
            v.encode('utf-8')
        except UnicodeEncodeError:
            return {'hex': os.fsencode(v).hex()}
        return v
    if isinstance(v, (list, tuple)):
        return [_encode(x) for x in v]
    if v is None or isinstance(v, (bool, int)):
        return v
    return _encode(str(v))


def dump(ns: argparse.Namespace) -> None:
    data = {k: _encode(v) for k, v in vars(ns).items()}
    sys.stdout.write(NS_MARKER + json.dumps(data, sort_keys=True) + '\n')
    sys.stdout.flush()
    os._exit(0)


class Const:
    """A non-`str` default (never passed through `type=`), rendered by `str()`."""

    def __init__(self, value: str) -> None:
        self.value = value

    def __str__(self) -> str:
        return self.value


def _type(name):
    if name is None:
        return None
    if name == 'use_color':
        return use_color
    if name == 'transform_stage':
        return transform_stage
    if name == 'int':
        return int
    if name == 'upper':
        def upper(s: str) -> str:
            if not s.isupper():
                raise ValueError(s)
            return s
        return upper
    if name == 'nonempty':
        def nonempty(s: str) -> str:
            if s == '':
                raise argparse.ArgumentTypeError('must not be empty')
            return s
        return nonempty

    def custom(s: str) -> str:
        return s

    custom.__name__ = name
    return custom


def _default(d):
    kind = d['kind']
    if kind == 'none':
        return None
    if kind == 'str':
        return d['value']
    if kind == 'env':
        return os.environ.get(d['var'], d['fallback'])
    if kind == 'false':
        return False
    if kind == 'true':
        return True
    if kind == 'int':
        return d['value']
    if kind == 'list':
        return []
    if kind == 'const':
        return Const(d['value'])
    raise ValueError(kind)


def _nargs(n):
    if n is None:
        return None
    if n == 0:
        return 0
    return {'?': '?', '*': '*', '+': '+', '...': argparse.REMAINDER, 'A...': argparse.PARSER}[n]


def _new_parser(spec: dict, prog: str, factory=argparse.ArgumentParser, **extra) -> argparse.ArgumentParser:
    return factory(
        prog=prog,
        description=spec['description'],
        epilog=spec['epilog'],
        add_help=spec['add_help'],
        allow_abbrev=spec['allow_abbrev'],
        **extra,
    )


def _add_actions(parser: argparse.ArgumentParser, spec: dict, prog: str) -> None:
    groups: dict[int, argparse._MutuallyExclusiveGroup] = {}
    for a in spec['actions']:
        kind = a['action']
        if kind == 'parsers':
            kw = {'dest': a['dest'], 'required': a['required']}
            if a['help'] is not None:
                kw['help'] = a['help']
            if a['metavar'] is not None:
                kw['metavar'] = a['metavar']
            sp = parser.add_subparsers(**kw)
            for s in a['subcommands']:
                sub_spec = s['parser']
                sub_prog = prog + sub_spec['prog_suffix']
                skw = {}
                if s['help'] is not None:
                    skw['help'] = s['help']
                sub = sp.add_parser(
                    s['name'],
                    prog=sub_prog,
                    description=sub_spec['description'],
                    epilog=sub_spec['epilog'],
                    add_help=sub_spec['add_help'],
                    allow_abbrev=sub_spec['allow_abbrev'],
                    **skw,
                )
                _add_actions(sub, sub_spec, sub_prog)
            continue
        target = parser
        if a['mutex_group'] is not None:
            if a['mutex_group'] not in groups:
                groups[a['mutex_group']] = parser.add_mutually_exclusive_group()
            target = groups[a['mutex_group']]
        kw = {}
        help_text = argparse.SUPPRESS if a['hidden'] else a['help']
        if help_text is not None:
            kw['help'] = help_text
        if kind == 'version':
            kw['action'] = 'version'
            d = a['default']
            kw['version'] = d.get('value')
            target.add_argument(*a['opts'], **kw)
            continue
        if kind == 'help':
            kw['action'] = 'help'
            target.add_argument(*a['opts'], **kw)
            continue
        kw['action'] = kind
        if kind in ('store', 'append'):
            n = _nargs(a['nargs'])
            if n is not None:
                kw['nargs'] = n
            if a['const'] is not None:
                kw['const'] = a['const']
            t = _type(a['type'])
            if t is not None:
                kw['type'] = t
            if a['choices'] is not None:
                kw['choices'] = a['choices']
            if a['metavar'] is not None:
                kw['metavar'] = a['metavar']
        if kind == 'store_const':
            kw['const'] = a['const']
        kw['default'] = _default(a['default'])
        if a['opts']:
            if kind in ('store', 'append', 'store_const', 'store_true', 'store_false'):
                kw['required'] = a['required']
            target.add_argument(*a['opts'], dest=a['dest'], **kw)
        else:
            target.add_argument(a['dest'], **kw)


def _build(path: str) -> argparse.ArgumentParser:
    with open(path, encoding='utf-8') as f:
        data = json.load(f)
    prog = data['prog']
    spec = data['parser']
    parser = _new_parser(spec, prog)
    _add_actions(parser, spec, prog)
    return parser


def main_json_plain(path: str, argv: list[str]) -> int:
    dump(_build(path).parse_args(argv))
    return 0


def main_json(path: str, argv: list[str]) -> int:
    parser = _build(path)
    if len(argv) == 0:
        argv = ['run']
    args = parser.parse_args(argv)
    if getattr(args, 'command', None) == 'help':
        if args.help_cmd:
            parser.parse_args([args.help_cmd, '--help'])
        else:
            parser.parse_args(['--help'])
        raise NotImplementedError('help re-parse returned')
    dump(args)
    return 0


def main_textwrap(path: str) -> int:
    import textwrap

    with open(path, encoding='utf-8') as f:
        cases = json.load(f)
    out = [
        textwrap.wrap(c['text'], c['width'], initial_indent=c['initial'], subsequent_indent=c['subsequent'])
        for c in cases
    ]
    sys.stdout.write(json.dumps(out) + '\n')
    return 0


def main_pre_commit(prog: str | None, argv: list[str]) -> int:
    if prog is not None:
        init = argparse.ArgumentParser.__init__

        def patched_init(self, *args, **kwargs):
            if kwargs.get('prog') == 'pre-commit':
                kwargs['prog'] = prog
            init(self, *args, **kwargs)

        argparse.ArgumentParser.__init__ = patched_init

    import pre_commit.color

    pre_commit.color.use_color = use_color

    parse_args = argparse.ArgumentParser.parse_args

    def patched_parse_args(self, args=None, namespace=None):
        ns = parse_args(self, args, namespace)
        if getattr(ns, 'command', None) != 'help':
            dump(ns)
        return ns

    argparse.ArgumentParser.parse_args = patched_parse_args

    from pre_commit.main import main

    return main(argv)


def main() -> int:
    args = sys.argv[1:]
    try:
        sep = args.index('--')
    except ValueError:
        raise SystemExit('usage: argparse_from_json.py json SPEC -- ARGV... | pre-commit [--prog P] -- ARGV...')
    head, argv = args[:sep], args[sep + 1:]
    if head[:1] == ['json'] and len(head) == 2:
        return main_json(head[1], argv)
    if head[:1] == ['json-plain'] and len(head) == 2:
        return main_json_plain(head[1], argv)
    if head[:1] == ['textwrap'] and len(head) == 2:
        return main_textwrap(head[1])
    if head[:1] == ['pre-commit']:
        prog = None
        if head[1:2] == ['--prog'] and len(head) == 3:
            prog = head[2]
        elif len(head) != 1:
            raise SystemExit('bad arguments')
        return main_pre_commit(prog, argv)
    raise SystemExit('bad arguments')


if __name__ == '__main__':
    raise SystemExit(main())
