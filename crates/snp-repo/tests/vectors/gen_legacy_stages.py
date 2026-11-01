import sys, os, tempfile
from pre_commit import clientlib
import pre_commit.clientlib as cl
cases = [
 b"-   id: a\n    stages: [push, manual]\n-   id: b\n    stages:\n    - commit\n    -   'merge-commit'\n    - push\n",
 b"-   id: a\n    stages: [pre-commit, pre-push]\n    name: commit push\n    args: [commit]\n",
 b"-   id: a\n    stages: commit\n",
 b"-   id: a\n    stages: [commit, 1]\n",
 b"-   id: a\n    stages: [commit, true]\n",
 b"-   id: a\n    stages: [commit, 'true']\n",
 b"-   id: a\n    stages:\n-   id: b\n    stages: [commit]\n",
 b"- stages: [\n    commit,  # old\n    push\n  ]\n  id: a\n",
 b"-   stages:\n    - merge-commit\n    id: a\n",
 b"-   id: a\n    'stages': [\"commit\"] # c\n    # stages: [push]\n",
 b"-   stages: [commit]\n",
 b"-   id: 1\n    stages: [commit]\n",
 b"-   id: a\n    stages: []\n-   id: b\n    stages: [push]\n",
 b"-   id: a\n    stages: [push]\n-   id: b\n    stages: [commit, push]\n",
 b"[]\n",
 b"{}\n",
 b"",
 b"-   id: a\n    stages: [commit]\n    stages: [push]\n",
 b"-   id: a\n    stages: [commit]\n-   [x]\n",
 b"-   id: a\n    stages: [\"push\", 'commit']\n",
 b"-   id: a\n    stages: [commit\n",
 b"-   id: a\n    stages:\n      - push\n      - commit\n    name: x\n",
 b"-   id: a\n    stages: [ commit , push ]\n",
 b"\xef\xbb\xbf-   id: a\n    stages: [commit]\n",
 b"-   id: a\n    stages: [commit]\n\t\n",
 b"-   id: a\n    stages: [~]\n",
 b"-   id: a\n    stages: [Commit]\n",
 b"-   id: a\n    stages: [commit] \n    files: x\n",
 b"-   id: 'a'\n    \"stages\":\n    -   commit\n",
 b"- id: a\n  stages:\n  - push\n- id: b\n  stages:\n  - commit\n",
 b"-   id: commit\n    name: push\n",
 b"-   id: a\n    stages: [commit, push, merge-commit, commit]\n",
 b"-   id: a\n    stages: [yes]\n",
 b"-   id: a\n    stages: [0x1]\n",
 b"-   id: a\n    stages: [commit]\n---\n- id: b\n",
 b"-   id: a\n    args:\n    -   commit\n    stages: [push]\n",
 b"-   id: a\n    args:\n    - commit\n    stages:\n    - push\n",
 b"-   id: a\n    x:\n        stages: [commit]\n",
 b"-   id: a\n    stages: [commit]\n  bad: x\n",
 b"-   id:\n    stages: [commit]\n",
 b"-   id: ''\n    stages: [commit]\n",
 b"-   id: \"a\"\n    stages: ['push']\n",
 b"-   id: a\n    stages: [commit]\n-   id: b\n",
 b"-   id: a\n    stages: [commit]\n-   name: b\n",
 b"  - id: a\n    stages: [push]\n",
 b"-   id: a\n    entry: 'x: y'\n    stages: [commit]\n",
 b"-   id: a\n    stages: [commit]\n    description: >\n        push\n",
 b"# comment\n-   id: a  # c\n    stages: [commit]  # c\n",
 b"-   id: a\n    stages: [merge-commit]\n\n\n-   id: b\n    stages:\n        - push\n",
]
import glob
for f in sorted(glob.glob(sys.argv[1] + '/*.yaml')):
    cases.append(open(f,'rb').read())
out = []
for c in cases:
    d = tempfile.mkdtemp()
    with open(os.path.join(d, '.pre-commit-hooks.yaml'), 'wb') as fh:
        fh.write(c)
    msgs = []
    orig = cl.logger.warning
    cl.logger.warning = lambda m: msgs.append(m)
    try:
        cl.warn_for_stages_on_repo_init('r', d)
    finally:
        cl.logger.warning = orig
    stages = ''
    if msgs:
        m = msgs[0]
        stages = m[m.index('(')+1:m.index(')')].replace(', ', ',')
    out.append(c.hex() + '\t' + stages)
sys.stdout.write('\n'.join(out) + '\n')
