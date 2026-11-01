// Prints Go's os.Expand (the grammar of os.ExpandEnv, which lefthook applies to job env values) for every
// input line, with a fixed environment, as JSON lines {"in": ..., "out": ...} (tests/vectors/expandenv.jsonl).
package main

import (
	"bufio"
	"encoding/json"
	"os"
)

var env = map[string]string{"A": "x y", "H": "/home/u", "AH": "$A`q`", "x": "\\\"", "A1": "1"}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	for sc.Scan() {
		var in string
		if err := json.Unmarshal(sc.Bytes(), &in); err != nil {
			panic(err)
		}
		out := os.Expand(in, func(k string) string { return env[k] })
		b, _ := json.Marshal(map[string]string{"in": in, "out": out})
		w.Write(b)
		w.WriteByte('\n')
	}
}
