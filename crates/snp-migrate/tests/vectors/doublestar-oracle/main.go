// Prints bmatcuk/doublestar v4.10.2 (lefthook's pinned matcher) verdicts as JSON lines for
// tests/vectors/doublestar.jsonl: m[i] = lefthook's `err == nil && matched` for doublestar.Match(p, paths[i])
// (both sides lowercased first when "lower", as filter.go does for `glob`); ok = doublestar.ValidatePattern(p).
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
)

type in struct {
	Pattern string   `json:"p"`
	Lower   bool     `json:"lower"`
	Paths   []string `json:"paths"`
}
type out struct {
	Pattern string   `json:"p"`
	Lower   bool     `json:"lower"`
	Ok      bool     `json:"ok"`
	Paths   []string `json:"paths"`
	Matches []bool   `json:"m"`
}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	for sc.Scan() {
		var i in
		if err := json.Unmarshal(sc.Bytes(), &i); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		p := i.Pattern
		if i.Lower {
			p = strings.ToLower(p)
		}
		o := out{Pattern: i.Pattern, Lower: i.Lower, Paths: i.Paths, Ok: doublestar.ValidatePattern(p)}
		for _, s := range i.Paths {
			if i.Lower {
				s = strings.ToLower(s)
			}
			m, err := doublestar.Match(p, s)
			o.Matches = append(o.Matches, err == nil && m)
		}
		b, _ := json.Marshal(o)
		w.Write(b)
		w.WriteByte('\n')
	}
}
