//! `snp_argparse::textwrap` against CPython 3.13's `textwrap.wrap` on 3,000 generated texts (ASCII and Unicode
//! letters, Unicode digits, hyphens and em-dashes, punctuation, every whitespace `TextWrapper` handles), widths 1–30
//! and assorted indents (12 §A.16). Vectors: `tests/vectors/textwrap.json`, written by `cargo xtask cli-goldens`.
//!
//! Owner: WP-12.

#[cfg(test)]
mod textwrap_vectors {
    use serde_json::Value;
    use snp_argparse::textwrap;

    #[test]
    fn wrap_and_fill_match_cpython() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/textwrap.json"),
        )
        .unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() >= 3000);
        let mut failures = Vec::new();
        for (i, c) in cases.iter().enumerate() {
            let input = c["text"].as_str().unwrap();
            let width = usize::try_from(c["width"].as_u64().unwrap()).unwrap();
            let initial = c["initial"].as_str().unwrap();
            let subsequent = c["subsequent"].as_str().unwrap();
            let want: Vec<String> = c["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect();
            let filled = textwrap::fill(input, width, initial, subsequent);
            if filled != want.join("\n") {
                failures.push(format!(
                    "#{i} fill({input:?}, {width}, {initial:?}, {subsequent:?})\n  want {want:?}\n  got  {filled:?}"
                ));
            }
            if initial.is_empty() && subsequent.is_empty() {
                let wrapped = textwrap::wrap(input, width);
                if wrapped != want {
                    failures.push(format!(
                        "#{i} wrap({input:?}, {width})\n  want {want:?}\n  got  {wrapped:?}"
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
