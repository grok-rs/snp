//! Iterative `Drop`, `Clone`, `PartialEq` and `Debug` for [`JsonValue`]: `json::loads` legally returns trees about
//! 10 000 levels deep, and the derived (recursive) impls overflow a 256 KiB stack on them.
//!
//! Owner: WP-17.

use std::fmt;

use super::JsonValue;
use super::value::split_tail;

impl Drop for JsonValue {
    fn drop(&mut self) {
        fn take(v: &mut JsonValue, stack: &mut Vec<JsonValue>) {
            match v {
                JsonValue::Array(items) => stack.append(items),
                JsonValue::Object(pairs) => {
                    stack.extend(std::mem::take(pairs).into_iter().map(|(_, x)| x));
                }
                _ => {}
            }
        }
        let mut stack: Vec<Self> = Vec::new();
        take(self, &mut stack);
        while let Some(mut v) = stack.pop() {
            take(&mut v, &mut stack);
        }
    }
}

impl Clone for JsonValue {
    fn clone(&self) -> Self {
        enum Work<'a> {
            Visit(&'a JsonValue),
            List(usize),
            Dict(Vec<String>),
        }
        let mut work = vec![Work::Visit(self)];
        let mut out: Vec<Self> = Vec::new();
        while let Some(w) = work.pop() {
            match w {
                Work::Visit(v) => match v {
                    Self::Null => out.push(Self::Null),
                    Self::Bool(b) => out.push(Self::Bool(*b)),
                    Self::Int(i) => out.push(Self::Int(i.clone())),
                    Self::Float(f) => out.push(Self::Float(*f)),
                    Self::Str(s) => out.push(Self::Str(s.clone())),
                    Self::Array(items) => {
                        work.push(Work::List(items.len()));
                        work.extend(items.iter().rev().map(Work::Visit));
                    }
                    Self::Object(pairs) => {
                        work.push(Work::Dict(pairs.iter().map(|(k, _)| k.clone()).collect()));
                        work.extend(pairs.iter().rev().map(|(_, v)| Work::Visit(v)));
                    }
                },
                Work::List(n) => {
                    let items = split_tail(&mut out, n);
                    out.push(Self::Array(items));
                }
                Work::Dict(keys) => {
                    let vals = split_tail(&mut out, keys.len());
                    out.push(Self::Object(keys.into_iter().zip(vals).collect()));
                }
            }
        }
        out.pop().unwrap_or(Self::Null)
    }
}

impl PartialEq for JsonValue {
    fn eq(&self, other: &Self) -> bool {
        let mut work: Vec<(&Self, &Self)> = vec![(self, other)];
        while let Some((a, b)) = work.pop() {
            match (a, b) {
                (Self::Null, Self::Null) => {}
                (Self::Bool(x), Self::Bool(y)) if x == y => {}
                (Self::Int(x), Self::Int(y)) if x == y => {}
                (Self::Float(x), Self::Float(y)) if x == y => {}
                (Self::Str(x), Self::Str(y)) if x == y => {}
                (Self::Array(x), Self::Array(y)) if x.len() == y.len() => {
                    work.extend(x.iter().zip(y));
                }
                (Self::Object(x), Self::Object(y)) if x.len() == y.len() => {
                    for ((kx, vx), (ky, vy)) in x.iter().zip(y) {
                        if kx != ky {
                            return false;
                        }
                        work.push((vx, vy));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl fmt::Debug for JsonValue {
    /// The derived compact form (`Array([Int("1"), Str("a")])`, `Object([("k", Null)])`), written iteratively;
    /// `{:#?}` prints the same compact form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        enum Work<'a> {
            Visit(&'a JsonValue),
            Key(&'a str),
            Text(&'static str),
        }
        let mut work = vec![Work::Visit(self)];
        while let Some(w) = work.pop() {
            match w {
                Work::Text(t) => f.write_str(t)?,
                Work::Key(k) => write!(f, "({k:?}, ")?,
                Work::Visit(v) => match v {
                    Self::Null => f.write_str("Null")?,
                    Self::Bool(b) => write!(f, "Bool({b:?})")?,
                    Self::Int(i) => write!(f, "Int({i:?})")?,
                    Self::Float(x) => write!(f, "Float({x:?})")?,
                    Self::Str(s) => write!(f, "Str({s:?})")?,
                    Self::Array(items) => {
                        f.write_str("Array([")?;
                        work.push(Work::Text("])"));
                        for (i, x) in items.iter().enumerate().rev() {
                            work.push(Work::Visit(x));
                            if i > 0 {
                                work.push(Work::Text(", "));
                            }
                        }
                    }
                    Self::Object(pairs) => {
                        f.write_str("Object([")?;
                        work.push(Work::Text("])"));
                        for (i, (k, x)) in pairs.iter().enumerate().rev() {
                            work.push(Work::Text(")"));
                            work.push(Work::Visit(x));
                            work.push(Work::Key(k));
                            if i > 0 {
                                work.push(Work::Text(", "));
                            }
                        }
                    }
                },
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PyMinor, loads};
    use super::*;

    /// Runs `f` on a thread with the fuzz harness' 256 KiB stack.
    fn small_stack(f: impl FnOnce() + Send + 'static) {
        let h = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .expect("spawn");
        assert!(h.join().is_ok());
    }

    #[test]
    fn deep_trees_drop_clone_eq_debug_on_small_stack() {
        small_stack(|| {
            for (open, close) in [("[", "]"), ("{\"a\":", "}")] {
                // 9990 levels: within the `loads` thresholds of both minors for arrays and objects.
                let rep = |s: &str| std::iter::repeat_n(s, 9990).collect::<String>();
                let doc = format!("{}1{}", rep(open), rep(close));
                for v in [PyMinor::P312, PyMinor::P313] {
                    let Ok(val) = loads(doc.as_bytes(), v) else {
                        panic!("{open} x 9990 must load");
                    };
                    let copy = val.clone();
                    assert_eq!(copy, val);
                    let o = super::super::DumpOpts {
                        indent: "1".to_owned(),
                        ensure_ascii: true,
                        sort_keys: true,
                        top_keys: vec!["a".to_owned()],
                    };
                    assert!(super::super::dumps_pretty(&val, &o).len() > 9990 * 2);
                    let text = format!("{val:?}");
                    assert!(text.len() > 9990 * 6);
                    drop(copy);
                    drop(val);
                }
            }
        });
    }

    #[test]
    fn debug_matches_the_derived_form() {
        let v = JsonValue::Object(vec![
            (
                "k".to_owned(),
                JsonValue::Array(vec![JsonValue::Int("1".into()), JsonValue::Null]),
            ),
            ("e".to_owned(), JsonValue::Array(Vec::new())),
            ("s".to_owned(), JsonValue::Str("\"x".to_owned())),
            ("f".to_owned(), JsonValue::Float(1.0)),
            ("b".to_owned(), JsonValue::Bool(true)),
        ]);
        assert_eq!(
            format!("{v:?}"),
            r#"Object([("k", Array([Int("1"), Null])), ("e", Array([])), ("s", Str("\"x")), ("f", Float(1.0)), ("b", Bool(true))])"#
        );
        assert_eq!(v.clone(), v);
        assert_ne!(JsonValue::Float(f64::NAN), JsonValue::Float(f64::NAN));
        assert_ne!(
            JsonValue::Array(vec![JsonValue::Null]),
            JsonValue::Array(Vec::new())
        );
        let mut o = v.clone();
        if let JsonValue::Object(p) = &mut o
            && let Some(first) = p.first_mut()
        {
            first.0 = "z".to_owned();
        }
        assert_ne!(o, v);
    }
}
