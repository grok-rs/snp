//! CPython `difflib.unified_diff` over `difflib.SequenceMatcher(None, a, b)` (`Lib/difflib.py`, identical in 3.12 and
//! 3.13; used by `pretty_format_json.get_diff` and `file_contents_sorter`).
//!
//! The matcher is ported operation for operation — Ratcliff/Obershelp longest matches with the `autojunk`
//! heuristic (in a `b` of 200 or more lines, a line occurring more than `len(b) // 100 + 1` times is "popular" and
//! cannot anchor a match) — because a different diff algorithm groups hunks differently. Lines are interned to
//! integers first; everything else follows the Python code.
//!
//! Owner: WP-17.

use std::collections::HashMap;

/// `''.join(difflib.unified_diff(a, b, fromfile=from, tofile=to))`.
///
/// `a` and `b` are the lines as `str.splitlines(True)` yields them (each keeps its line ending; a last line without
/// one is emitted as is, with no `\ No newline at end of file`). Header lines are `--- {from}\n` and
/// `+++ {to}\n` (no dates, no tab), hunks `@@ -{range} +{range} @@\n` with three lines of context. Identical
/// inputs give an empty string.
pub fn unified_diff(a: &[&str], b: &[&str], from: &str, to: &str) -> String {
    let (ai, bi) = intern(a, b);
    let opcodes = Matcher::new(&ai, &bi).opcodes();
    let mut out = String::new();
    let mut started = false;
    for group in grouped_opcodes(opcodes, 3) {
        if !started {
            started = true;
            push_all(&mut out, &["--- ", from, "\n+++ ", to, "\n"]);
        }
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        push_all(
            &mut out,
            &[
                "@@ -",
                &format_range(first.i1, last.i2),
                " +",
                &format_range(first.j1, last.j2),
                " @@\n",
            ],
        );
        for op in &group {
            if op.tag == Tag::Equal {
                for line in a.get(op.i1..op.i2).unwrap_or_default() {
                    push_all(&mut out, &[" ", line]);
                }
                continue;
            }
            if matches!(op.tag, Tag::Replace | Tag::Delete) {
                for line in a.get(op.i1..op.i2).unwrap_or_default() {
                    push_all(&mut out, &["-", line]);
                }
            }
            if matches!(op.tag, Tag::Replace | Tag::Insert) {
                for line in b.get(op.j1..op.j2).unwrap_or_default() {
                    push_all(&mut out, &["+", line]);
                }
            }
        }
    }
    out
}

fn push_all(out: &mut String, parts: &[&str]) {
    for p in parts {
        out.push_str(p);
    }
}

/// `_format_range_unified(start, stop)`.
fn format_range(start: usize, stop: usize) -> String {
    let mut beginning = start.saturating_add(1);
    let length = stop.saturating_sub(start);
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning = beginning.saturating_sub(1);
    }
    format!("{beginning},{length}")
}

/// Lines as small integers (equal lines, equal ids).
fn intern<'l>(a: &[&'l str], b: &[&'l str]) -> (Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<&'l str, u32> = HashMap::new();
    let mut id = |s: &'l str| -> u32 {
        let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        *ids.entry(s).or_insert(next)
    };
    let ai: Vec<u32> = a.iter().map(|&s| id(s)).collect();
    let bi: Vec<u32> = b.iter().map(|&s| id(s)).collect();
    (ai, bi)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

#[derive(Copy, Clone, Debug)]
struct Opcode {
    tag: Tag,
    i1: usize,
    i2: usize,
    j1: usize,
    j2: usize,
}

/// `SequenceMatcher(None, a, b, autojunk=True)`.
struct Matcher<'s> {
    a: &'s [u32],
    b: &'s [u32],
    b2j: HashMap<u32, Vec<usize>>,
}

impl<'s> Matcher<'s> {
    /// `__chain_b`: index `b`, then drop the popular elements (no `isjunk`, so no junk).
    fn new(a: &'s [u32], b: &'s [u32]) -> Self {
        let mut b2j: HashMap<u32, Vec<usize>> = HashMap::new();
        for (j, &x) in b.iter().enumerate() {
            b2j.entry(x).or_default().push(j);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = (n / 100).saturating_add(1);
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        Self { a, b, b2j }
    }

    fn at_a(&self, i: usize) -> Option<u32> {
        self.a.get(i).copied()
    }

    fn at_b(&self, j: usize) -> Option<u32> {
        self.b.get(j).copied()
    }

    /// `find_longest_match(alo, ahi, blo, bhi)` → (i, j, k).
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.at_a(i).and_then(|x| self.b2j.get(&x)) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let prev = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0);
                    let k = prev.saturating_add(1);
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i.saturating_add(1).saturating_sub(k);
                        bestj = j.saturating_add(1).saturating_sub(k);
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // Extend with equal elements on both sides (no junk: `isbjunk` is always false).
        while besti > alo
            && bestj > blo
            && self.at_a(besti.saturating_sub(1)) == self.at_b(bestj.saturating_sub(1))
        {
            besti = besti.saturating_sub(1);
            bestj = bestj.saturating_sub(1);
            bestsize = bestsize.saturating_add(1);
        }
        while besti.saturating_add(bestsize) < ahi
            && bestj.saturating_add(bestsize) < bhi
            && self.at_a(besti.saturating_add(bestsize))
                == self.at_b(bestj.saturating_add(bestsize))
        {
            bestsize = bestsize.saturating_add(1);
        }
        (besti, bestj, bestsize)
    }

    /// `get_matching_blocks()`, including the final `(len(a), len(b), 0)`.
    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0usize, la, 0usize, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i.saturating_add(k) < ahi && j.saturating_add(k) < bhi {
                    queue.push((i.saturating_add(k), ahi, j.saturating_add(k), bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0usize, 0usize, 0usize);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1.saturating_add(k1) == i2 && j1.saturating_add(k1) == j2 {
                k1 = k1.saturating_add(k2);
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }

    /// `get_opcodes()`.
    fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0usize, 0usize);
        let mut out = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                out.push(Opcode {
                    tag,
                    i1: i,
                    i2: ai,
                    j1: j,
                    j2: bj,
                });
            }
            i = ai.saturating_add(size);
            j = bj.saturating_add(size);
            if size > 0 {
                out.push(Opcode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: i,
                    j1: bj,
                    j2: j,
                });
            }
        }
        out
    }
}

/// `get_grouped_opcodes(n)`.
fn grouped_opcodes(mut codes: Vec<Opcode>, n: usize) -> Vec<Vec<Opcode>> {
    if codes.is_empty() {
        codes.push(Opcode {
            tag: Tag::Equal,
            i1: 0,
            i2: 1,
            j1: 0,
            j2: 1,
        });
    }
    if let Some(first) = codes.first_mut().filter(|c| c.tag == Tag::Equal) {
        first.i1 = first.i1.max(first.i2.saturating_sub(n));
        first.j1 = first.j1.max(first.j2.saturating_sub(n));
    }
    if let Some(last) = codes.last_mut().filter(|c| c.tag == Tag::Equal) {
        last.i2 = last.i2.min(last.i1.saturating_add(n));
        last.j2 = last.j2.min(last.j1.saturating_add(n));
    }
    let nn = n.saturating_add(n);
    let mut groups = Vec::new();
    let mut group: Vec<Opcode> = Vec::new();
    for mut op in codes {
        if op.tag == Tag::Equal && op.i2.saturating_sub(op.i1) > nn {
            group.push(Opcode {
                tag: Tag::Equal,
                i1: op.i1,
                i2: op.i2.min(op.i1.saturating_add(n)),
                j1: op.j1,
                j2: op.j2.min(op.j1.saturating_add(n)),
            });
            groups.push(std::mem::take(&mut group));
            op.i1 = op.i1.max(op.i2.saturating_sub(n));
            op.j1 = op.j1.max(op.j2.saturating_sub(n));
        }
        group.push(op);
    }
    let only_equal = group.len() == 1 && group.first().is_some_and(|g| g.tag == Tag::Equal);
    if !group.is_empty() && !only_equal {
        groups.push(group);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_without_newline() {
        // 09 §4.C: `{"a":1}` → pretty; no "\ No newline at end of file".
        let d = unified_diff(
            &["{\"a\":1}"],
            &["{\n", "  \"a\": 1\n", "}\n"],
            "f.json",
            "f.json",
        );
        assert_eq!(
            d,
            "--- f.json\n+++ f.json\n@@ -1 +1,3 @@\n-{\"a\":1}+{\n+  \"a\": 1\n+}\n"
        );
    }

    #[test]
    fn identical_is_empty() {
        assert_eq!(unified_diff(&["a\n"], &["a\n"], "x", "y"), "");
        assert_eq!(unified_diff(&[], &[], "x", "y"), "");
    }

    #[test]
    fn ranges() {
        assert_eq!(format_range(0, 0), "0,0");
        assert_eq!(format_range(3, 4), "4");
        assert_eq!(format_range(3, 6), "4,3");
        let d = unified_diff(&[], &["a\n"], "x", "y");
        assert_eq!(d, "--- x\n+++ y\n@@ -0,0 +1 @@\n+a\n");
    }
}
