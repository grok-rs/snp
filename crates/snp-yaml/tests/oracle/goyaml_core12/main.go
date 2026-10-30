// Command goyaml_core12 writes snp-yaml's Schema::Yaml12Core oracle vectors (tests/vectors/core12.json): what
// go-yaml v3 — the YAML library lefthook reads its config with (through koanf) — makes of `k: <scalar>`, plain and
// with an explicit `!!int`/`!!float`/`!!bool`/`!!null`/`!!str` tag.
//
// Regenerate (needs Go and the gopkg.in/yaml.v3 v3.0.1 module, whose resolve.go equals go.yaml.in/yaml/v3's):
//
//	cd crates/snp-yaml/tests/oracle/goyaml_core12 && go run . > ../../vectors/core12.json
//
// Owner: WP-14.
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"strconv"
	"time"

	"gopkg.in/yaml.v3"
)

type vector struct {
	Yaml string `json:"yaml"`
	// Kind is null, bool, int (int64 or uint64, Value is decimal), float (Value is the IEEE bits in hex),
	// str, time (Value is RFC 3339) or error.
	Kind  string `json:"kind"`
	Value string `json:"value"`
}

func corpus() []string {
	seen := map[string]bool{}
	var out []string
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for _, s := range []string{
		"", "~", "null", "Null", "NULL", "nULL", "true", "True", "TRUE", "tRue", "false", "False", "FALSE",
		"yes", "Yes", "no", "on", "On", "off", "y", "n", ".inf", ".Inf", ".INF", "+.inf", "-.inf", "-.Inf",
		"+.INF", ".nan", ".NaN", ".NAN", "+.nan", "-.nan", "inf", "nan", "Infinity", "<<", "=", "<", "1:30",
		"12:30:45", "190:20:30", "2020-01-01", "2020-1-1", "2020-01-01T10:00:00Z", "2020-01-01t10:00:00Z",
		"2020-01-01 10:00:00", "2001-12-14 21:59:43.10 -5", "2020-13-45", "20200-01-01", "v1.2.3", "1.2.3",
		"1.2.3.4", "0x1p3", "0x1.8p1", "1e400", "-1e400", "1e-400", ".5e400", "1.7976931348623157e308",
		"1.7976931348623159e308", "4.9e-324", "9223372036854775807", "9223372036854775808",
		"-9223372036854775808", "-9223372036854775809", "18446744073709551615", "18446744073709551616",
		"+18446744073709551615", "-18446744073709551615", "0xFFFFFFFFFFFFFFFF", "0x10000000000000000",
		"0x8000000000000000", "-0x8000000000000000", "-0x8000000000000001", "0o1777777777777777777777",
		"0b" + repeat("1", 64), "0b" + repeat("1", 65), "-0b" + repeat("1", 63), "-0b1" + repeat("0", 63),
		"0b-1", "0b+1", "-0b-1", "-0b+1", "0o-7", "0o+7", "-0o-7", "0b-1_0", "0o-1_7", "1" + repeat("0", 30),
		"0" + repeat("7", 22), "0" + repeat("7", 21), "1.5", "1.", ".5", "-.5", "+.5", ".", "-.", "1.5e3",
		"1.5E+3", "1.5e-3", "1e3", "1E3", "1e+3", "1e", "1e+", ".e3", ".5e", ".5e3", "._5", ".5_0", ".5__0",
		".5_", "._", ".5e1_0", ".5e_1", ".5e1_", "1_0.5", "1_0.5_0", "1.5_e3", "1__0", "1_", "_1", "__",
		"0x_1", "0x1_F", "0_x1", "0b_1", "0o_7", "+_1", "-_1", "1_e3", "+1", "-1", "+0", "-0", "0", "00",
		"000", "01", "07", "08", "09", "010", "0777", "0778", "0o17", "0O17", "0o8", "0o", "0x", "0b", "0x1F",
		"0X1F", "0xg", "0xG", "0b1", "0B1", "0b2", "+0x1F", "-0x1F", "+0b1", "-0b1", "+0o17", "-0o17", "+08",
		"-08", "-09.5", "08.5", "0e0", "00e1", "1 ", "1\t", "- 1", "-", "+", "+-1", "--1", "1-", "1+1",
		"0x1F.0", "1,000", "1 000", "１", "٣", "1 ", "123abc", "abc123", "0.0", "-0.0", "-0", "1_000",
		"1_000_000", "0b1_0", "0o1_7", "0x1_f", "1:0", "-1:30", "1.0:30",
	} {
		add(s)
	}
	for _, sign := range []string{"", "+", "-"} {
		for _, prefix := range []string{"", "0", "0x", "0X", "0o", "0O", "0b", "0B"} {
			for _, body := range []string{"0", "1", "7", "8", "9", "a", "F", "f", "10", "17", "19", "1_0",
				"_1", "1_", "1__0", "1.5", "1e3", "1.e3"} {
				add(sign + prefix + body)
			}
		}
	}
	return out
}

func repeat(s string, n int) string {
	out := ""
	for i := 0; i < n; i++ {
		out += s
	}
	return out
}

func encode(v interface{}) (string, string) {
	switch x := v.(type) {
	case nil:
		return "null", ""
	case bool:
		return "bool", strconv.FormatBool(x)
	case int:
		return "int", strconv.FormatInt(int64(x), 10)
	case int64:
		return "int", strconv.FormatInt(x, 10)
	case uint64:
		return "int", strconv.FormatUint(x, 10)
	case float64:
		return "float", fmt.Sprintf("%016x", math.Float64bits(x))
	case string:
		return "str", x
	case time.Time:
		return "time", x.Format(time.RFC3339Nano)
	default:
		return "other", fmt.Sprintf("%T %v", v, v)
	}
}

func main() {
	var vecs []vector
	for _, s := range corpus() {
		quoted, _ := json.Marshal(s)
		docs := []string{"k: " + s + "\n"}
		for _, tag := range []string{"!!int", "!!float", "!!bool", "!!null", "!!str"} {
			docs = append(docs, "k: "+tag+" "+string(quoted)+"\n")
		}
		for _, doc := range docs {
			var m map[string]interface{}
			if err := yaml.Unmarshal([]byte(doc), &m); err != nil {
				vecs = append(vecs, vector{Yaml: doc, Kind: "error", Value: err.Error()})
				continue
			}
			v, ok := m["k"]
			if !ok {
				continue
			}
			kind, value := encode(v)
			vecs = append(vecs, vector{Yaml: doc, Kind: kind, Value: value})
		}
	}
	fmt.Println(`{"meta": {"generator": "goyaml_core12", "library": "gopkg.in/yaml.v3 v3.0.1"}, "vectors": [`)
	for i, v := range vecs {
		b, _ := json.Marshal(v)
		os.Stdout.Write(b)
		if i+1 < len(vecs) {
			fmt.Println(",")
		} else {
			fmt.Println()
		}
	}
	fmt.Println("]}")
}
