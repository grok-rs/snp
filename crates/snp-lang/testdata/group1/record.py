# snp WP-24 oracle recorder: runs pre-commit 4.7.0 in-process with its process layer faked for the language tools
# and appends one line per faked spawn to a golden log (format v1 of crates/snp-lang/src/harness: argv with the tool
# basename first, cwd, env delta against a base env).
#
# Usage: <oracle>/bin/python -I record.py <scenario.json>
#
# scenario.json:
#   argv      pre-commit's argv (e.g. ["run", "--all-files"])
#   base_env  the env delta base (the env pre-commit was started with, plus PRE_COMMIT=1)
#   fake      tool basenames whose spawns are faked and logged (git and everything else runs for real)
#   replies   [{"tool", "when": [args prefix], "status", "stdout", "stderr", "effects": [...]}], first match wins
#   log       the log file
#   go_stub   true: golang's `_install_go` only creates `<env>/.go/bin` (no download)
#
# Effects (applied before answering): {"mkdir": T}, {"exe": T} (empty executable file), {"write": [T, text]};
# templates T expand {cwd}, {argN} (argv index, 0 = the tool) and {env:NAME}.
#
# A faked command is not resolved on PATH (parse_shebang.normalize_cmd returns it unchanged), so the logged argv is
# exactly what pre-commit hands to its process layer, and no fake tool has to exist. Generated for
# crates/snp-lang/tests/group1/oracle.rs; do not edit the recorded goldens by hand.
import json
import os
import subprocess
import sys


def main():
    with open(sys.argv[1], encoding='utf-8') as f:
        sc = json.load(f)
    fake = set(sc['fake'])
    base = sc['base_env']
    log_path = sc['log']

    import pre_commit.parse_shebang as parse_shebang
    import pre_commit.util as util
    import pre_commit.xargs as xargs

    def tool_of(cmd):
        return os.path.basename(str(cmd[0]))

    real_normalize = parse_shebang.normalize_cmd

    def normalize_cmd(cmd, *, env=None):
        if cmd and tool_of(cmd) in fake:
            return tuple(cmd)
        return real_normalize(cmd, env=env)

    parse_shebang.normalize_cmd = normalize_cmd

    def expand(t, argv, cwd, env):
        out = t.replace('{cwd}', cwd)
        for i, a in enumerate(argv):
            out = out.replace('{arg%d}' % i, a)
        while '{env:' in out:
            start = out.index('{env:')
            end = out.index('}', start)
            name = out[start + 5:end]
            out = out[:start] + env.get(name, '') + out[end + 1:]
        return out

    def apply(effects, argv, cwd, env):
        for e in effects:
            if 'mkdir' in e:
                os.makedirs(expand(e['mkdir'], argv, cwd, env), exist_ok=True)
            elif 'exe' in e:
                p = expand(e['exe'], argv, cwd, env)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, 'w'):
                    pass
                os.chmod(p, 0o755)
            elif 'write' in e:
                p = expand(e['write'][0], argv, cwd, env)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, 'w', encoding='utf-8') as f:
                    f.write(e['write'][1])

    class Fake:
        def __init__(self, argv, kwargs):
            env = kwargs.get('env')
            env = dict(os.environ if env is None else env)
            cwd = kwargs.get('cwd') or os.getcwd()
            tool = tool_of(argv)
            delta = {}
            for k in sorted(set(env) | set(base)):
                if k not in env:
                    delta[k] = None
                elif base.get(k) != env[k]:
                    delta[k] = env[k]
            record = {'argv': [tool, *[str(a) for a in argv[1:]]], 'cwd': str(cwd), 'env': delta}
            with open(log_path, 'a', encoding='utf-8') as f:
                f.write(json.dumps(record, ensure_ascii=False, separators=(',', ':')) + '\n')
            self.returncode = 0
            self.stdout = b''
            self.stderr = b''
            args = [str(a) for a in argv[1:]]
            for r in sc['replies']:
                when = r.get('when', [])
                if r['tool'] == tool and args[:len(when)] == when:
                    apply(r.get('effects', []), [str(a) for a in argv], str(cwd), env)
                    self.returncode = r.get('status', 0)
                    self.stdout = r.get('stdout', '').encode()
                    self.stderr = r.get('stderr', '').encode()
                    break
            self.merge = kwargs.get('stderr') == subprocess.STDOUT

        def communicate(self, input=None, timeout=None):
            if self.merge:
                return self.stdout + self.stderr, None
            return self.stdout, self.stderr

    real_popen = subprocess.Popen

    def popen(argv, *args, **kwargs):
        if argv and tool_of(argv) in fake:
            return Fake(list(argv), kwargs)
        return real_popen(argv, *args, **kwargs)

    util.subprocess.Popen = popen
    assert xargs.subprocess is subprocess

    if sc.get('go_stub'):
        import pre_commit.languages.golang as golang

        def install_go(version, dest):
            os.makedirs(os.path.join(dest, '.go', 'bin'), exist_ok=True)

        golang._install_go = install_go

    from pre_commit.main import main as pc_main
    return pc_main(sc['argv'])


if __name__ == '__main__':
    raise SystemExit(main())
