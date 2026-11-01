# Generates random gobwas patterns and candidate paths (JSON lines for the Go oracle).
import json, random, sys
rnd = random.Random(int(sys.argv[1]))
n = int(sys.argv[2])
TOK = ["a","b","B","c","z","Z",".","/","-","+","!","^","]","}",",","é","İ","K",
       "*","**","***","?","[ab]","[!a]","[a-c]","[.]","[ab-z]","[a-z0-9]","[a-]","[]","[!]","[xa-c]",
       "[]-a]","[\\]a-]","[\\-a]","[!^x]","[^]","[[:]","[[]","[!!]","[a\\-z]","[A-Z]","[é]","[!-a]",
       "{a,b}","{x*,y,}","{a,{b,c}}","{}","{,}","{a","{a,b","}","\\*","\\a","\\","\\[","\\{","a\\","{[a-c],?}"]
CH = "abcABzZ.-/+*é!^]}[,İKx"
def pat():
    return "".join(rnd.choice(TOK) for _ in range(rnd.randint(1,6)))
def inst(p):
    # a rough instantiation of the pattern so that matches are frequent
    out=[]
    i=0
    while i < len(p):
        c=p[i]
        if c=='*': out.append("".join(rnd.choice(CH) for _ in range(rnd.randint(0,3))))
        elif c=='?': out.append(rnd.choice(CH))
        elif c in '[]{},\\!': 
            if rnd.random()<0.5: out.append(rnd.choice(CH))
        else: out.append(c)
        i+=1
    return "".join(out)
for _ in range(n):
    p = pat()
    paths = set()
    for _ in range(5): paths.add(inst(p))
    for _ in range(3): paths.add("".join(rnd.choice(CH) for _ in range(rnd.randint(0,5))))
    for lower in (True, False):
        print(json.dumps({"p":p,"lower":lower,"paths":sorted(paths)}, ensure_ascii=False))
