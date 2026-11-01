# Generates doublestar patterns and candidate paths for the Go oracle:
# python3 gen.py 34 900 | (cd doublestar-oracle && go run .) > doublestar.jsonl
import json, random, sys
rnd = random.Random(int(sys.argv[1]))
n = int(sys.argv[2])
TOK = ["a", "b", "B", "src", "x", ".", "-", "é", "/", "/", "/", "**", "**/", "/**", "/**/", "*", "*", "?",
       "[ab]", "[!a]", "[^a]", "[a-c]", "[!/]", "{a,b}", "{x*,y}", "{a,b/c}", "\\*", "\\a", ".rb", "*.js"]
SEG = ["a", "b", "B", "src", "x", "x.rb", "a.js", ".", "é", "ab", "c"]
FIXED = ["x[!a]y", "x[^a]y", "src/**/", "src/**", "**/*.rb", "src/*.js", "**", "a/**/b", "**/", "/**/x"]
def pat():
    return "".join(rnd.choice(TOK) for _ in range(rnd.randint(1, 6)))
def inst(p):
    out = []
    for c in p:
        if c == "*":
            out.append(rnd.choice(["", "a", "b/c", "x.rb", "/", "ab"]))
        elif c == "?":
            out.append(rnd.choice(["a", "/", "é"]))
        elif c in "[]{},\\!^":
            if rnd.random() < 0.5:
                out.append(rnd.choice("ab/x"))
        else:
            out.append(c)
    return "".join(out)
def rand_path():
    return "/".join(rnd.choice(SEG) for _ in range(rnd.randint(1, 4)))
for k in range(len(FIXED) + n):
    p = FIXED[k] if k < len(FIXED) else pat()
    paths = set()
    for _ in range(6):
        q = inst(p)
        paths.add(q)
        paths.add(q.rstrip("/"))
    for _ in range(4):
        paths.add(rand_path())
    paths.update(["x/y", "xby", "src", "src/a", "src/a/b", "a/b", "a", "b/x.rb"])
    paths.discard("")
    for lower in (True, False):
        print(json.dumps({"p": p, "lower": lower, "paths": sorted(paths)}, ensure_ascii=False))
