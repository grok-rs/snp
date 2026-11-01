# Generates env values for the Go os.Expand oracle: python3 gen.py SEED N | (cd expandenv-oracle && go run .)
import json, random, sys
rnd = random.Random(int(sys.argv[1]))
n = int(sys.argv[2])
TOK = ["$", "{", "}", "A", "H", "1", "#", "@", "-", ":", "x", "/", '"', "\\", "`", " ", "?", "*", "_",
       "$A", "${A}", "${H}", "$H/bin", "${A:-d}", "${#H}", "$1", "${1}", "$@", "$$", "$-", "${}", "${", "$0",
       "${10}", "$AH", "${x}", "$(id)", "é", "'"]
fixed = ["${HOME:-d}", "${#HOME}", "$1", "$@", "$$", "$-", "${}", "${", "$", "a$", "$A1x", "$Ax", "${A}x",
         "$*", "${*}", "${!}", "$?", "${?}", "${-}", "${#}", "$#", "${a-b}", "${A", "$A$H", "$$A"]
seen = set()
for s in fixed + ["".join(rnd.choice(TOK) for _ in range(rnd.randint(1, 7))) for _ in range(n)]:
    if s not in seen:
        seen.add(s)
        print(json.dumps(s, ensure_ascii=False))
