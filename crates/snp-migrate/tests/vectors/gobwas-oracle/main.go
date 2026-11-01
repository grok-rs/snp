package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/gobwas/glob"
)

type in struct {
	Pattern string   `json:"p"`
	Lower   bool     `json:"lower"`
	Paths   []string `json:"paths"`
}
type out struct {
	Pattern string `json:"p"`
	Lower   bool   `json:"lower"`
	Ok      bool   `json:"ok"`
	Paths   []string `json:"paths"`
	Matches []bool `json:"m"`
}

func compile(p string) (g glob.Glob, ok bool) {
	defer func() {
		if r := recover(); r != nil {
			ok = false
		}
	}()
	g = glob.MustCompile(p)
	return g, true
}

func match(g glob.Glob, s string) (m bool, ok bool) {
	defer func() {
		if r := recover(); r != nil {
			ok = false
		}
	}()
	return g.Match(s), true
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
		o := out{Pattern: i.Pattern, Lower: i.Lower, Paths: i.Paths}
		g, ok := compile(p)
		o.Ok = ok
		if ok {
			for _, s := range i.Paths {
				if i.Lower {
					s = strings.ToLower(s)
				}
				m, mok := match(g, s)
				if !mok {
					o.Ok = false
					o.Matches = nil
					break
				}
				o.Matches = append(o.Matches, m)
			}
		}
		b, _ := json.Marshal(o)
		w.Write(b)
		w.WriteByte('\n')
	}
}
