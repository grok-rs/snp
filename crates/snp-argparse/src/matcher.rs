//! `ArgumentParser._match_argument` / `_match_arguments_partial` (`argparse.py:2313-2350`) without regular
//! expressions.
//!
//! argparse encodes the argument strings as a pattern over `O` (an option), `A` (an argument) and `-` (the first
//! `--`), builds one regex group per action from `_get_nargs_pattern` and calls `re.match` on the concatenation.
//! Every group is a sequence of greedy quantified character classes, so the backtracking result is: at each atom,
//! the largest count for which the rest of the pattern can still match. [`match_seq`] computes exactly that with a
//! suffix-feasibility table (`O(atoms × len)`), which keeps parsing linear on adversarial argv.
//!
//! Owner: WP-12.

use crate::Nargs;

/// A character class of the pattern alphabet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Class {
    /// `A`
    A,
    /// `-`
    Dash,
    /// `[A-]`
    ADash,
    /// `[AO]`
    AO,
    /// `.` / `[-AO]`
    Any,
}

impl Class {
    const fn accepts(self, c: u8) -> bool {
        match self {
            Self::A => c == b'A',
            Self::Dash => c == b'-',
            Self::ADash => c == b'A' || c == b'-',
            Self::AO => c == b'A' || c == b'O',
            Self::Any => true,
        }
    }
}

/// One greedy quantified class: `class{min,max}` (`max == None`: unbounded).
#[derive(Copy, Clone, Debug)]
struct Atom {
    class: Class,
    min: usize,
    max: Option<usize>,
}

const fn star(class: Class) -> Atom {
    Atom {
        class,
        min: 0,
        max: None,
    }
}

const fn one(class: Class) -> Atom {
    Atom {
        class,
        min: 1,
        max: Some(1),
    }
}

const OPT_A: Atom = Atom {
    class: Class::A,
    min: 0,
    max: Some(1),
};

/// `_get_nargs_pattern` of a positional (no option strings).
fn positional_atoms(nargs: Nargs) -> &'static [Atom] {
    const ONE: [Atom; 3] = [star(Class::Dash), one(Class::A), star(Class::Dash)];
    const OPTIONAL: [Atom; 3] = [star(Class::Dash), OPT_A, star(Class::Dash)];
    const ZERO_OR_MORE: [Atom; 2] = [star(Class::Dash), star(Class::ADash)];
    const ONE_OR_MORE: [Atom; 3] = [star(Class::Dash), one(Class::A), star(Class::ADash)];
    const REMAINDER: [Atom; 1] = [star(Class::Any)];
    const PARSER: [Atom; 3] = [star(Class::Dash), one(Class::A), star(Class::Any)];
    const ZERO: [Atom; 1] = [star(Class::Dash)];
    match nargs {
        Nargs::One => &ONE,
        Nargs::Optional => &OPTIONAL,
        Nargs::ZeroOrMore => &ZERO_OR_MORE,
        Nargs::OneOrMore => &ONE_OR_MORE,
        Nargs::Remainder => &REMAINDER,
        Nargs::Parser => &PARSER,
        Nargs::Zero => &ZERO,
    }
}

/// "No position" in the feasibility table of [`match_seq`].
const NONE: usize = usize::MAX;

/// `re.match(''.join(patterns), pattern)` for the positional `nargs` in order: the group lengths and the end of the
/// match, or `None` when it does not match.
pub(crate) fn match_seq(nargs: &[Nargs], pattern: &[u8]) -> Option<(Vec<usize>, usize)> {
    let atoms: Vec<(usize, Atom)> = nargs
        .iter()
        .enumerate()
        .flat_map(|(g, &n)| positional_atoms(n).iter().map(move |&a| (g, a)))
        .collect();
    let n = pattern.len();
    let width = n.saturating_add(1);
    let m = atoms.len();
    // `last[i * width + q]`: the largest r <= q from which atoms i.. can match (`NONE`: no such r). Row m (no
    // atoms left) matches everywhere: the match is anchored at the start only.
    let mut last = vec![NONE; m.saturating_add(1).saturating_mul(width)];
    for (q, slot) in last
        .iter_mut()
        .skip(m.saturating_mul(width))
        .take(width)
        .enumerate()
    {
        *slot = q;
    }
    // `hi[pos]` of the current atom: pos + its greedy maximum (run of its class, capped by its max).
    let mut hi = vec![0usize; width];
    for i in (0..m).rev() {
        let Some(&(_, atom)) = atoms.get(i) else {
            continue;
        };
        let mut run = 0usize;
        for pos in (0..width).rev() {
            run = match pattern.get(pos) {
                Some(&c) if atom.class.accepts(c) => run.saturating_add(1),
                _ => 0,
            };
            if let Some(h) = hi.get_mut(pos) {
                *h = pos.saturating_add(atom.max.map_or(run, |mx| mx.min(run)));
            }
        }
        let next_row = i.saturating_add(1).saturating_mul(width);
        let row = i.saturating_mul(width);
        let mut best = NONE;
        for pos in 0..width {
            let lo = pos.saturating_add(atom.min);
            let h = hi.get(pos).copied().unwrap_or(0);
            let reach = last
                .get(next_row.saturating_add(h))
                .copied()
                .unwrap_or(NONE);
            if lo <= h && reach != NONE && reach >= lo {
                best = pos;
            }
            if let Some(slot) = last.get_mut(row.saturating_add(pos)) {
                *slot = best;
            }
        }
    }
    if last.first().copied() != Some(0) {
        return None;
    }
    let mut counts = vec![0usize; nargs.len()];
    let mut pos = 0usize;
    for (i, &(g, atom)) in atoms.iter().enumerate() {
        let run = pattern
            .get(pos..)
            .unwrap_or_default()
            .iter()
            .take_while(|&&c| atom.class.accepts(c))
            .count();
        let h = pos.saturating_add(atom.max.map_or(run, |mx| mx.min(run)));
        let to = last
            .get(i.saturating_add(1).saturating_mul(width).saturating_add(h))
            .copied()
            .filter(|&r| r != NONE)?;
        let c = to.checked_sub(pos)?;
        if let Some(slot) = counts.get_mut(g) {
            *slot = slot.saturating_add(c);
        }
        pos = to;
    }
    Some((counts, pos))
}

/// `_match_arguments_partial`: match as many positionals as possible (dropping from the right); trailing
/// zero-length matches are given back when the match stops right before an option.
pub(crate) fn match_partial(nargs: &[Nargs], pattern: &[u8]) -> Vec<usize> {
    for i in (1..=nargs.len()).rev() {
        if let Some((mut counts, end)) = match_seq(nargs.get(..i).unwrap_or_default(), pattern) {
            if pattern.get(end) == Some(&b'O') {
                while counts.last() == Some(&0) {
                    counts.pop();
                }
            }
            return counts;
        }
    }
    Vec::new()
}

/// The error of `_match_argument` for an option whose `nargs` pattern does not match.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NargsError(pub(crate) &'static str);

/// `_match_argument` for an option (patterns `([A])`, `(A?)`, `(A*)`, `(A+)`, `([AO]*)`, `(A[AO]*)`, `()`): the
/// number of argument strings it takes from `pattern`.
pub(crate) fn match_option(nargs: Nargs, pattern: &[u8]) -> Result<usize, NargsError> {
    let count = |class: Class| pattern.iter().take_while(|&&c| class.accepts(c)).count();
    let first_a = pattern.first() == Some(&b'A');
    match nargs {
        Nargs::Zero => Ok(0),
        Nargs::One if first_a => Ok(1),
        Nargs::One => Err(NargsError("expected one argument")),
        Nargs::Optional => Ok(usize::from(first_a)),
        Nargs::ZeroOrMore => Ok(count(Class::A)),
        Nargs::OneOrMore if first_a => Ok(count(Class::A)),
        Nargs::OneOrMore => Err(NargsError("expected at least one argument")),
        Nargs::Remainder => Ok(count(Class::AO)),
        Nargs::Parser if first_a => Ok(count(Class::AO)),
        // `ngettext('expected %s argument', 'expected %s arguments', 'A...')`
        Nargs::Parser => Err(NargsError("expected A... arguments")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Nargs::*;

    #[test]
    fn partial_matches_like_re() {
        // try-repo REPO -a HOOK: the `?` hook positional is given back before the option (3.12.7+/3.13.1+).
        assert_eq!(match_partial(&[One, Optional], b"AOA"), vec![1]);
        assert_eq!(match_partial(&[One, Optional], b"AA"), vec![1, 1]);
        assert_eq!(match_partial(&[One, Optional], b""), Vec::<usize>::new());
        assert_eq!(match_partial(&[One, Remainder], b""), Vec::<usize>::new());
        assert_eq!(match_partial(&[One, Remainder], b"AOA-A"), vec![1, 4]);
        assert_eq!(match_partial(&[Parser], b"-AOA"), vec![4]);
        assert_eq!(match_partial(&[Parser], b"-"), Vec::<usize>::new());
        assert_eq!(match_partial(&[ZeroOrMore], b"AA-AO"), vec![4]);
        assert_eq!(match_partial(&[Optional], b"-A"), vec![2]);
        assert_eq!(match_partial(&[Optional], b"AA"), vec![1]);
        assert_eq!(match_partial(&[Optional], b"O"), Vec::<usize>::new());
        assert_eq!(match_partial(&[ZeroOrMore, One], b"AAAA"), vec![3, 1]);
        assert_eq!(match_partial(&[One, ZeroOrMore], b"A-AA"), vec![2, 2]);
        assert_eq!(match_partial(&[OneOrMore, One], b"A"), vec![1]);
        assert_eq!(match_partial(&[One, One], b"A-A"), vec![2, 1]);
    }

    #[test]
    fn option_counts() {
        assert_eq!(match_option(One, b"AA"), Ok(1));
        assert_eq!(
            match_option(One, b"O"),
            Err(NargsError("expected one argument"))
        );
        assert_eq!(
            match_option(One, b"-A"),
            Err(NargsError("expected one argument"))
        );
        assert_eq!(match_option(ZeroOrMore, b"AAOA"), Ok(2));
        assert_eq!(match_option(ZeroOrMore, b"-A"), Ok(0));
        assert_eq!(match_option(Remainder, b"AOA-A"), Ok(3));
        assert_eq!(match_option(Optional, b""), Ok(0));
    }
}
