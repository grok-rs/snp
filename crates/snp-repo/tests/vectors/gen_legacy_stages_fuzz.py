"""Differential corpus for crates/snp-repo/src/stages.rs: generated manifests labelled by pre-commit 4.7.0's
clientlib.warn_for_stages_on_repo_init. Two deterministic generators (seed 1): the WP-31 review's corpus (4000
documents; every 4th warned document, every 50th silent one) and a mixed-feature one (20000 documents: anchors,
aliases, tags, flow mappings, markers, directives, CR line ends, tabs, block scalars; every warned document, every
50th silent one). Regenerate (do not edit legacy_stages_fuzz.tsv):
  target/oracle/pc470-py312/bin/python -I crates/snp-repo/tests/vectors/gen_legacy_stages_fuzz.py \
    > crates/snp-repo/tests/vectors/legacy_stages_fuzz.tsv
"""
import os, random, sys, tempfile
import pre_commit.clientlib as cl

# --- the review generator ---
def _review():
    global R
    import random, sys
    R = random.Random(1)
    STG = ['commit','push','merge-commit','pre-commit','manual',"'commit'",'"push"','Commit','commit ','yes','1']
    def flow(items, multi):
        if multi: return '[\n' + ',\n'.join('      '+i for i in items) + '\n    ]'
        return '[' + ', '.join(items) + ']'
    def hook(ind, style):
        keys = []
        if R.random() < 0.95: keys.append(('id', R.choice(['a','"b"',"'c'",'x-y','commit'])))
        if R.random() < 0.8:
            items = R.sample(STG[:7], R.randint(0,3))
            r = R.random()
            if r < 0.45: keys.append(('stages', flow(items, R.random()<0.15)))
            elif r < 0.9 and items: keys.append(('stages', ('BLOCK', items)))
            else: keys.append(('stages', flow(items, False)))
        for k in R.sample(['name: n','entry: e','language: python','args: [commit]','description: "x: y"','files: \\.py$','types: [python]'], R.randint(0,3)):
            a,b = k.split(': ',1); keys.append((a,b))
        if R.random() < 0.1: keys.append(('description', ('BLOCKSCALAR', 'push\n')))
        if R.random() < 0.05: keys.append(('stages', flow(['push'], False)))
        R.shuffle(keys)
        sub = ' ' * ind
        out = []
        dash_alone = R.random() < 0.08
        first = True
        for k, v in keys:
            if R.random() < 0.05: k = "'%s'" % k
            if isinstance(v, tuple) and v[0] == 'BLOCK':
                bi = R.choice([0, 2, 4])
                body = k + ':\n' + ''.join(sub + ' '*bi + '- ' + it + '\n' for it in v[1])
                body = body[:-1]
            elif isinstance(v, tuple):
                body = k + ': |\n' + sub + '    ' + v[1].rstrip('\n')
            else:
                body = k + ': ' + v
            if R.random() < 0.1: body = body.replace('\n', '  # cmt\n', 1) if '\n' in body else body + '  # c'
            if first and not dash_alone:
                out.append('-' + ' '*(ind-1) + body); first = False
            else:
                if first: out.append('-'); first = False
                out.append(sub + body)
        if not keys: out.append('- {}')
        return '\n'.join(out)
    def review_docs(n):
        out = []
        for _ in range(n):
            ind = R.choice([2,4])
            hooks = [hook(ind, 0) for _ in range(R.randint(1,3))]
            sep = R.choice(['\n','\n','\n\n','\n# c\n'])
            doc = sep.join(hooks) + '\n'
            r = R.random()
            if r < 0.08: doc = '---\n' + doc
            elif r < 0.12: doc = '# yaml\n---\n' + doc
            elif r < 0.15: doc = doc.replace('\n', '\r\n')
            elif r < 0.17: doc = doc.replace('\n', '\r')
            elif r < 0.19: doc = doc + '...\n'
            elif r < 0.21: doc = '%YAML 1.1\n---\n' + doc
            elif r < 0.23: doc = doc + '\t\n'
            elif r < 0.25: doc = ' ' * R.choice([1,2]) + doc.replace('\n', '\n' + ' ')
            out.append(doc.encode())
        return out
    return review_docs
# --- the mixed-feature generator ---
def _mixed():
    global R
    R = random.Random(1)
    SC = ['commit','push','merge-commit','pre-commit','manual',"'commit'",'"push"','"comm\\x69t"','!!str commit','&A commit','*A','-','a:','a: b','commit:x','yes','1','~','',"'it''s'",'x #y','-x','[commit]','{a: b}','- commit','? x','commit  ','!!int 1','!foo x','"a\\qb"','>-\n  commit','|\n  x']
    IDS = ['a','"b"','x-y','1','null','a:','&I a','!!str 1','[a]','{a: 1}','*A','a b','"a" x']
    def scal(L): return R.choice(L)
    def seq_block(ind):
        items = [scal(SC) for _ in range(R.randint(0,3))]
        sub = ' '*(ind + R.choice([0,2,4]))
        if not items: return ' []'
        return '\n' + '\n'.join(sub + R.choice(['- ','-   ','-\t','-']) + i.replace('\n', '\n'+sub+'  ') for i in items)
    def seq_flow():
        items = [scal(SC) for _ in range(R.randint(0,3))]
        sep = R.choice([', ', ',', ' ,', ',\n      ', ',\t'])
        t = ' [' + sep.join(items) + R.choice([']', ',]', ' ]', '\n    ]'])
        if R.random() < 0.1: t = ' !!seq' + t
        if R.random() < 0.05: t = ' &S' + t
        return t
    def hook_block(top, dash):
        ind = top + len(dash)
        keys = []
        if R.random() < 0.92: keys.append('id: ' + scal(IDS))
        for _ in range(R.choice([0,1,1,1,2])):
            v = seq_flow() if R.random() < 0.5 else (seq_block(ind) if R.random() < 0.85 else ' ' + R.choice(['*S','commit','null','','!!seq []']))
            keys.append(R.choice(['stages','stages','"stages"','stages ']) + ':' + v)
        for k in R.sample(['name: n','entry: e','args: [commit]','description: x\n'+' '*(ind+2)+'y','x: |\n'+' '*(ind+2)+'stages: [push]','files: \\.py$','y:\n'+' '*ind+'- push'], R.randint(0,2)):
            keys.append(k)
        R.shuffle(keys)
        if not keys: return ' '*top + '- {}'
        lines = []
        for i, k in enumerate(keys):
            if R.random() < 0.05: k = k + R.choice(['  # c', '\t# c', ' #c', '\t'])
            if i == 0:
                if R.random() < 0.08:
                    lines.append(' '*top + '-' + R.choice(['', ' &H', ' !!map', '  # c'])); lines.append(' '*ind + k)
                else:
                    lines.append(' '*top + dash + R.choice(['', '', '&K ']) + k)
            else:
                lines.append(' '*ind + k)
        return '\n'.join(lines)
    def hook_flow(top):
        parts = []
        if R.random() < 0.9: parts.append('id: ' + scal(['a','"b"','1','a b']))
        if R.random() < 0.8: parts.append('stages:' + seq_flow())
        if R.random() < 0.3: parts.append('name: n')
        R.shuffle(parts)
        return ' '*top + '- {' + ', '.join(parts) + '}'
    def doc():
        top = R.choice([0,0,0,2])
        dash = R.choice(['- ', '-   ', '-  '])
        hs = []
        for _ in range(R.randint(1,3)):
            r = R.random()
            if r < 0.15: hs.append(hook_flow(top))
            elif r < 0.2 and hs: hs.append(' '*top + '- *H')
            else: hs.append(hook_block(top, dash))
        s = R.choice(['\n', '\n\n', '\n# c\n', '\n  \n']).join(hs) + R.choice(['\n', '', '\n\n'])
        r = R.random()
        if r < 0.05: s = '---\n' + s
        elif r < 0.08: s = '--- # c\n' + s
        elif r < 0.10: s = '%YAML 1.1\n---\n' + s
        elif r < 0.12: s = s + '...\n'
        elif r < 0.14: s = s.replace('\n', '\r\n')
        elif r < 0.16: s = s.replace('\n', '\r')
        elif r < 0.17: s = s + '---\n- id: z\n'
        elif r < 0.19: s = '--- [' + ', '.join('{id: a, stages: [%s]}' % scal(['commit','push','x']) for _ in range(2)) + ']\n'
        return s.encode()
    return doc

d = tempfile.mkdtemp()
p = os.path.join(d, '.pre-commit-hooks.yaml')
msgs = []
cl.logger.warning = lambda m: msgs.append(m)


def label(c):
    with open(p, 'wb') as fh:
        fh.write(c)
    msgs.clear()
    cl.warn_for_stages_on_repo_init('r', d)
    if not msgs:
        return ''
    m = msgs[0]
    return m[m.index('(') + 1:m.index(')')].replace(', ', ',')


rows = []
review_docs = _review()
warned = silent = 0
for c in review_docs(4000):
    s = label(c)
    if s:
        warned += 1
        if warned % 4 == 1:
            rows.append((c, s))
    else:
        silent += 1
        if silent % 50 == 1:
            rows.append((c, s))
doc = _mixed()
silent = 0
for _ in range(20000):
    c = doc()
    s = label(c)
    if s:
        rows.append((c, s))
    else:
        silent += 1
        if silent % 50 == 1:
            rows.append((c, s))
sys.stdout.write('# pre-commit 4.7.0 (pc470-py312) verdicts on generated manifests: <hex>\t<warned legacy stages>. '
                 'Generated by gen_legacy_stages_fuzz.py; do not edit.\n')
sys.stdout.write(''.join(c.hex() + '\t' + s + '\n' for c, s in rows))

