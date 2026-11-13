//! CPython's `list.sort()` (`Objects/listobject.c`: timsort with powersort merging, galloping, binary insertion),
//! ported comparison for comparison so that a comparator that raises (`TypeError`) or is inconsistent (`NaN`)
//! fails or orders exactly as CPython does. 3.12 and 3.13 differ in run detection (`count_run`); the merge
//! machinery is the same.
//!
//! The port sorts a permutation of indices and applies it at the end; every array access is checked (an
//! impossible out-of-range index degrades to a stable insertion sort instead of panicking).
//!
//! Owner: WP-17.

use crate::json::PyMinor;

/// `MIN_GALLOP`.
const MIN_GALLOP: usize = 7;

/// Why a sort stopped.
enum Stop<E> {
    /// The comparator raised.
    Raised(E),
    /// An index escaped its run (impossible by construction).
    Internal,
}

impl<E> From<E> for Stop<E> {
    fn from(e: E) -> Self {
        Self::Raised(e)
    }
}

/// `items.sort()` with `lt` as Python's `<`; `Err` is the first exception a comparison raised (CPython leaves the
/// list permuted then; callers treat it as the crash it is).
pub(crate) fn sort<T, E>(
    items: &mut Vec<T>,
    v: PyMinor,
    mut lt: impl FnMut(&T, &T) -> Result<bool, E>,
) -> Result<(), E> {
    let n = items.len();
    if n < 2 {
        return Ok(());
    }
    let mut keys: Vec<usize> = (0..n).collect();
    let result = {
        let mut ms = State {
            lt: |a: usize, b: usize| -> Result<bool, Stop<E>> {
                match (items.get(a), items.get(b)) {
                    (Some(x), Some(y)) => lt(x, y).map_err(Stop::Raised),
                    _ => Err(Stop::Internal),
                }
            },
            min_gallop: MIN_GALLOP,
            pending: Vec::new(),
            v,
        };
        ms.run(&mut keys)
    };
    match result {
        Ok(()) => {}
        Err(Stop::Raised(e)) => return Err(e),
        Err(Stop::Internal) => {
            // A stable binary-insertion sort: unlike `slice::sort_by`, it cannot panic on the inconsistent orders
            // (NaN, mixed types) that `lt` may describe.
            keys = (0..n).collect();
            for i in 1..n {
                let Some(&key) = keys.get(i) else { break };
                let (mut lo, mut hi) = (0, i);
                while lo < hi {
                    let mid = lo.midpoint(hi);
                    let less = match (items.get(key), keys.get(mid).and_then(|&at| items.get(at))) {
                        (Some(held), Some(placed)) => lt(held, placed).unwrap_or(false),
                        _ => false,
                    };
                    if less {
                        hi = mid;
                    } else {
                        lo = mid.saturating_add(1);
                    }
                }
                if let Some(run) = keys.get_mut(lo..=i) {
                    run.rotate_right(1);
                }
            }
        }
    }
    apply_permutation(items, &keys);
    Ok(())
}

/// Reorders `items` so that `items[i]` becomes the old `items[keys[i]]`.
fn apply_permutation<T>(items: &mut Vec<T>, keys: &[usize]) {
    let mut slots: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
    for &k in keys {
        if let Some(x) = slots.get_mut(k).and_then(Option::take) {
            items.push(x);
        }
    }
    items.extend(slots.into_iter().flatten());
}

/// How a merge loop ended (`Succeed:` or the `CopyA:`/`CopyB:` shortcut).
#[derive(Copy, Clone, Debug)]
enum Exit {
    Done,
    Copy,
}

/// A pending run: start index, length and powersort power.
#[derive(Copy, Clone, Debug)]
struct Run {
    base: usize,
    len: usize,
    power: u32,
}

struct State<F> {
    lt: F,
    min_gallop: usize,
    pending: Vec<Run>,
    v: PyMinor,
}

fn get(a: &[usize], i: usize) -> Option<usize> {
    a.get(i).copied()
}

fn set(a: &mut [usize], i: usize, x: usize) -> Option<()> {
    *a.get_mut(i)? = x;
    Some(())
}

fn internal<E>() -> Stop<E> {
    Stop::Internal
}

impl<E, F: FnMut(usize, usize) -> Result<bool, Stop<E>>> State<F> {
    /// `IFLT(x, y)` on positions `i`, `j` of `a`.
    fn lt_at(&mut self, keys: &[usize], left: usize, right: usize) -> Result<bool, Stop<E>> {
        let (lhs, rhs) = (
            get(keys, left).ok_or_else(internal)?,
            get(keys, right).ok_or_else(internal)?,
        );
        (self.lt)(lhs, rhs)
    }

    /// `list_sort_impl`'s main loop.
    fn run(&mut self, a: &mut [usize]) -> Result<(), Stop<E>> {
        let n = a.len();
        let minrun = compute_minrun(n);
        let mut lo = 0usize;
        let mut nremaining = n;
        while nremaining > 0 {
            let mut run = match self.v {
                PyMinor::P312 => {
                    self.count_run_312(a, lo, lo.checked_add(nremaining).ok_or_else(internal)?)?
                }
                PyMinor::P313 => self.count_run_313(a, lo, nremaining)?,
            };
            if run < minrun {
                let force = nremaining.min(minrun);
                self.binarysort(
                    a,
                    lo,
                    lo.checked_add(force).ok_or_else(internal)?,
                    lo.checked_add(run).ok_or_else(internal)?,
                )?;
                run = force;
            }
            self.found_new_run(a, run, n)?;
            self.pending.push(Run {
                base: lo,
                len: run,
                power: 0,
            });
            lo = lo.checked_add(run).ok_or_else(internal)?;
            nremaining = nremaining.checked_sub(run).ok_or_else(internal)?;
        }
        self.force_collapse(a)
    }

    /// 3.12 `binarysort(lo, hi, start)`: `a[lo..start]` is sorted; insert `a[start..hi]`.
    fn binarysort(
        &mut self,
        a: &mut [usize],
        lo: usize,
        hi: usize,
        mut start: usize,
    ) -> Result<(), Stop<E>> {
        if lo == start {
            start = start.checked_add(1).ok_or_else(internal)?;
        }
        while start < hi {
            let pivot = get(a, start).ok_or_else(internal)?;
            let (mut l, mut r) = (lo, start);
            while l < r {
                let p = l
                    .checked_add(r.checked_sub(l).ok_or_else(internal)? >> 1)
                    .ok_or_else(internal)?;
                let ap = get(a, p).ok_or_else(internal)?;
                if (self.lt)(pivot, ap)? {
                    r = p;
                } else {
                    l = p.checked_add(1).ok_or_else(internal)?;
                }
            }
            let mut p = start;
            while p > l {
                let prev = get(a, p.checked_sub(1).ok_or_else(internal)?).ok_or_else(internal)?;
                set(a, p, prev).ok_or_else(internal)?;
                p = p.checked_sub(1).ok_or_else(internal)?;
            }
            set(a, l, pivot).ok_or_else(internal)?;
            start = start.checked_add(1).ok_or_else(internal)?;
        }
        Ok(())
    }

    /// 3.12 `count_run(lo, hi, &descending)`; a strictly descending run is reversed here.
    fn count_run_312(&mut self, a: &mut [usize], lo: usize, hi: usize) -> Result<usize, Stop<E>> {
        let mut i = lo.checked_add(1).ok_or_else(internal)?;
        if i == hi {
            return Ok(1);
        }
        let mut n = 2usize;
        if self.lt_at(a, i, i.checked_sub(1).ok_or_else(internal)?)? {
            i = i.checked_add(1).ok_or_else(internal)?;
            while i < hi {
                if self.lt_at(a, i, i.checked_sub(1).ok_or_else(internal)?)? {
                    i = i.checked_add(1).ok_or_else(internal)?;
                    n = n.checked_add(1).ok_or_else(internal)?;
                } else {
                    break;
                }
            }
            reverse(a, lo, lo.checked_add(n).ok_or_else(internal)?).ok_or_else(internal)?;
        } else {
            i = i.checked_add(1).ok_or_else(internal)?;
            while i < hi {
                if self.lt_at(a, i, i.checked_sub(1).ok_or_else(internal)?)? {
                    break;
                }
                i = i.checked_add(1).ok_or_else(internal)?;
                n = n.checked_add(1).ok_or_else(internal)?;
            }
        }
        Ok(n)
    }

    /// 3.13 `count_run(slo, nremaining)`: ascending runs, or descending runs with all-equal sub-runs (reversed in
    /// place so the final reversal keeps stability), extended by a following ascending stretch.
    fn count_run_313(
        &mut self,
        a: &mut [usize],
        lo: usize,
        nremaining: usize,
    ) -> Result<usize, Stop<E>> {
        let at = |k: usize| lo.checked_add(k).ok_or_else(internal::<E>);
        let mut n = 1usize;
        // IF_NEXT_SMALLER: lo[n] < lo[n-1]
        while n < nremaining {
            if self.lt_at(a, at(n)?, at(n.checked_sub(1).ok_or_else(internal)?)?)? {
                break;
            }
            n = n.checked_add(1).ok_or_else(internal)?;
        }
        if n == nremaining {
            return Ok(n);
        }
        if n > 1 {
            if self.lt_at(a, at(0)?, at(n.checked_sub(1).ok_or_else(internal)?)?)? {
                return Ok(n);
            }
            reverse(a, lo, at(n)?).ok_or_else(internal)?;
        }
        n = n.checked_add(1).ok_or_else(internal)?;
        let mut neq = 0usize;
        while n < nremaining {
            let (cur, prev) = (at(n)?, at(n.checked_sub(1).ok_or_else(internal)?)?);
            if self.lt_at(a, cur, prev)? {
                if neq > 0 {
                    let k = neq.checked_add(1).ok_or_else(internal)?;
                    let s = at(n.checked_sub(k).ok_or_else(internal)?)?;
                    reverse(a, s, s.checked_add(k).ok_or_else(internal)?).ok_or_else(internal)?;
                    neq = 0;
                }
            } else if self.lt_at(a, prev, cur)? {
                break;
            } else {
                neq = neq.checked_add(1).ok_or_else(internal)?;
            }
            n = n.checked_add(1).ok_or_else(internal)?;
        }
        if neq > 0 {
            let k = neq.checked_add(1).ok_or_else(internal)?;
            let s = at(n.checked_sub(k).ok_or_else(internal)?)?;
            reverse(a, s, s.checked_add(k).ok_or_else(internal)?).ok_or_else(internal)?;
        }
        reverse(a, lo, at(n)?).ok_or_else(internal)?;
        while n < nremaining {
            if self.lt_at(a, at(n)?, at(n.checked_sub(1).ok_or_else(internal)?)?)? {
                break;
            }
            n = n.checked_add(1).ok_or_else(internal)?;
        }
        Ok(n)
    }

    /// `gallop_left(key, a[base..base+n], hint)`: the k with `a[k-1] < key <= a[k]`.
    fn gallop_left(
        &mut self,
        key: usize,
        a: &[usize],
        base: usize,
        n: usize,
        hint: usize,
    ) -> Result<usize, Stop<E>> {
        let at = |i: usize| -> Result<usize, Stop<E>> {
            get(a, base.checked_add(i).ok_or_else(internal)?).ok_or_else(internal)
        };
        let mut lastofs: usize = 0;
        let mut ofs: usize = 1;
        let (mut lo_ofs, mut hi_ofs): (isize, usize);
        if (self.lt)(at(hint)?, key)? {
            let maxofs = n.checked_sub(hint).ok_or_else(internal)?;
            while ofs < maxofs {
                if (self.lt)(at(hint.checked_add(ofs).ok_or_else(internal)?)?, key)? {
                    lastofs = ofs;
                    ofs = ofs
                        .checked_shl(1)
                        .and_then(|x| x.checked_add(1))
                        .ok_or_else(internal)?;
                } else {
                    break;
                }
            }
            ofs = ofs.min(maxofs);
            lo_ofs = isize::try_from(lastofs.checked_add(hint).ok_or_else(internal)?)
                .map_err(|_| internal())?;
            hi_ofs = ofs.checked_add(hint).ok_or_else(internal)?;
        } else {
            let maxofs = hint.checked_add(1).ok_or_else(internal)?;
            while ofs < maxofs {
                if (self.lt)(at(hint.checked_sub(ofs).ok_or_else(internal)?)?, key)? {
                    break;
                }
                lastofs = ofs;
                ofs = ofs
                    .checked_shl(1)
                    .and_then(|x| x.checked_add(1))
                    .ok_or_else(internal)?;
            }
            ofs = ofs.min(maxofs);
            let hint_i = isize::try_from(hint).map_err(|_| internal())?;
            let ofs_i = isize::try_from(ofs).map_err(|_| internal())?;
            lo_ofs = hint_i.checked_sub(ofs_i).ok_or_else(internal)?;
            hi_ofs = hint.checked_sub(lastofs).ok_or_else(internal)?;
        }
        lo_ofs = lo_ofs.checked_add(1).ok_or_else(internal)?;
        let mut lo = usize::try_from(lo_ofs).map_err(|_| internal())?;
        while lo < hi_ofs {
            let m = lo
                .checked_add(hi_ofs.checked_sub(lo).ok_or_else(internal)? >> 1)
                .ok_or_else(internal)?;
            if (self.lt)(at(m)?, key)? {
                lo = m.checked_add(1).ok_or_else(internal)?;
            } else {
                hi_ofs = m;
            }
        }
        Ok(hi_ofs)
    }

    /// `gallop_right(key, a[base..base+n], hint)`: the k with `a[k-1] <= key < a[k]`.
    fn gallop_right(
        &mut self,
        key: usize,
        a: &[usize],
        base: usize,
        n: usize,
        hint: usize,
    ) -> Result<usize, Stop<E>> {
        let at = |i: usize| -> Result<usize, Stop<E>> {
            get(a, base.checked_add(i).ok_or_else(internal)?).ok_or_else(internal)
        };
        let mut lastofs: usize = 0;
        let mut ofs: usize = 1;
        let (mut lo_ofs, mut hi_ofs): (isize, usize);
        if (self.lt)(key, at(hint)?)? {
            let maxofs = hint.checked_add(1).ok_or_else(internal)?;
            while ofs < maxofs {
                if (self.lt)(key, at(hint.checked_sub(ofs).ok_or_else(internal)?)?)? {
                    lastofs = ofs;
                    ofs = ofs
                        .checked_shl(1)
                        .and_then(|x| x.checked_add(1))
                        .ok_or_else(internal)?;
                } else {
                    break;
                }
            }
            ofs = ofs.min(maxofs);
            let hint_i = isize::try_from(hint).map_err(|_| internal())?;
            let ofs_i = isize::try_from(ofs).map_err(|_| internal())?;
            lo_ofs = hint_i.checked_sub(ofs_i).ok_or_else(internal)?;
            hi_ofs = hint.checked_sub(lastofs).ok_or_else(internal)?;
        } else {
            let maxofs = n.checked_sub(hint).ok_or_else(internal)?;
            while ofs < maxofs {
                if (self.lt)(key, at(hint.checked_add(ofs).ok_or_else(internal)?)?)? {
                    break;
                }
                lastofs = ofs;
                ofs = ofs
                    .checked_shl(1)
                    .and_then(|x| x.checked_add(1))
                    .ok_or_else(internal)?;
            }
            ofs = ofs.min(maxofs);
            lo_ofs = isize::try_from(lastofs.checked_add(hint).ok_or_else(internal)?)
                .map_err(|_| internal())?;
            hi_ofs = ofs.checked_add(hint).ok_or_else(internal)?;
        }
        lo_ofs = lo_ofs.checked_add(1).ok_or_else(internal)?;
        let mut lo = usize::try_from(lo_ofs).map_err(|_| internal())?;
        while lo < hi_ofs {
            let m = lo
                .checked_add(hi_ofs.checked_sub(lo).ok_or_else(internal)? >> 1)
                .ok_or_else(internal)?;
            if (self.lt)(key, at(m)?)? {
                hi_ofs = m;
            } else {
                lo = m.checked_add(1).ok_or_else(internal)?;
            }
        }
        Ok(hi_ofs)
    }

    /// `merge_lo`: merge `a[pa..pa+na]` with `a[pb..pb+nb]` (`pb == pa + na`, `na <= nb`), run A copied out.
    /// A comparator exception returns at once (the list's state no longer matters to the caller).
    fn merge_lo(
        &mut self,
        a: &mut [usize],
        pa: usize,
        mut na: usize,
        pb: usize,
        mut nb: usize,
    ) -> Result<(), Stop<E>> {
        let end_a = pa.checked_add(na).ok_or_else(internal)?;
        let tmp: Vec<usize> = a.get(pa..end_a).ok_or_else(internal)?.to_vec();
        let inc = |x: usize| x.checked_add(1).ok_or_else(internal::<E>);
        let dec = |x: usize| x.checked_sub(1).ok_or_else(internal::<E>);
        let mut ta = 0usize; // current A element (in tmp)
        let mut ib = pb; // current B element (in a)
        let mut dest = pa; // next slot to fill
        let exit = 'merge: {
            set(a, dest, get(a, ib).ok_or_else(internal)?).ok_or_else(internal)?;
            dest = inc(dest)?;
            ib = inc(ib)?;
            nb = dec(nb)?;
            if nb == 0 {
                break 'merge Exit::Done;
            }
            if na == 1 {
                break 'merge Exit::Copy;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut count_a = 0usize;
                let mut count_b = 0usize;
                loop {
                    let x = get(a, ib).ok_or_else(internal)?;
                    let y = get(&tmp, ta).ok_or_else(internal)?;
                    if (self.lt)(x, y)? {
                        set(a, dest, x).ok_or_else(internal)?;
                        dest = inc(dest)?;
                        ib = inc(ib)?;
                        count_b = inc(count_b)?;
                        count_a = 0;
                        nb = dec(nb)?;
                        if nb == 0 {
                            break 'merge Exit::Done;
                        }
                        if count_b >= min_gallop {
                            break;
                        }
                    } else {
                        set(a, dest, y).ok_or_else(internal)?;
                        dest = inc(dest)?;
                        ta = inc(ta)?;
                        count_a = inc(count_a)?;
                        count_b = 0;
                        na = dec(na)?;
                        if na == 1 {
                            break 'merge Exit::Copy;
                        }
                        if count_a >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop = inc(min_gallop)?;
                loop {
                    if min_gallop > 1 {
                        min_gallop = dec(min_gallop)?;
                    }
                    self.min_gallop = min_gallop;
                    let key = get(a, ib).ok_or_else(internal)?;
                    let k = self.gallop_right(key, &tmp, ta, na, 0)?;
                    count_a = k;
                    if k > 0 {
                        for _ in 0..k {
                            set(a, dest, get(&tmp, ta).ok_or_else(internal)?)
                                .ok_or_else(internal)?;
                            dest = inc(dest)?;
                            ta = inc(ta)?;
                        }
                        na = na.checked_sub(k).ok_or_else(internal)?;
                        if na == 1 {
                            break 'merge Exit::Copy;
                        }
                        if na == 0 {
                            break 'merge Exit::Done;
                        }
                    }
                    set(a, dest, get(a, ib).ok_or_else(internal)?).ok_or_else(internal)?;
                    dest = inc(dest)?;
                    ib = inc(ib)?;
                    nb = dec(nb)?;
                    if nb == 0 {
                        break 'merge Exit::Done;
                    }
                    let key = get(&tmp, ta).ok_or_else(internal)?;
                    let k = self.gallop_left(key, a, ib, nb, 0)?;
                    count_b = k;
                    if k > 0 {
                        for _ in 0..k {
                            set(a, dest, get(a, ib).ok_or_else(internal)?).ok_or_else(internal)?;
                            dest = inc(dest)?;
                            ib = inc(ib)?;
                        }
                        nb = nb.checked_sub(k).ok_or_else(internal)?;
                        if nb == 0 {
                            break 'merge Exit::Done;
                        }
                    }
                    set(a, dest, get(&tmp, ta).ok_or_else(internal)?).ok_or_else(internal)?;
                    dest = inc(dest)?;
                    ta = inc(ta)?;
                    na = dec(na)?;
                    if na == 1 {
                        break 'merge Exit::Copy;
                    }
                    if count_a < MIN_GALLOP && count_b < MIN_GALLOP {
                        break;
                    }
                }
                min_gallop = inc(min_gallop)?;
                self.min_gallop = min_gallop;
            }
        };
        match exit {
            // `Succeed:` what is left of A goes to the end.
            Exit::Done => {
                for _ in 0..na {
                    set(a, dest, get(&tmp, ta).ok_or_else(internal)?).ok_or_else(internal)?;
                    dest = inc(dest)?;
                    ta = inc(ta)?;
                }
            }
            // `CopyB:` na == 1: the rest of B moves down, A's last element goes after it.
            Exit::Copy => {
                for _ in 0..nb {
                    set(a, dest, get(a, ib).ok_or_else(internal)?).ok_or_else(internal)?;
                    dest = inc(dest)?;
                    ib = inc(ib)?;
                }
                set(a, dest, get(&tmp, ta).ok_or_else(internal)?).ok_or_else(internal)?;
            }
        }
        Ok(())
    }

    /// `merge_hi`: merge `a[pa..pa+na]` with `a[pb..pb+nb]` (`na >= nb`) from the right, run B copied out.
    /// Positions `dest`, `ia`, `tb` point one past the slot/element in use, so they never go below zero.
    fn merge_hi(
        &mut self,
        a: &mut [usize],
        pa: usize,
        mut na: usize,
        pb: usize,
        mut nb: usize,
    ) -> Result<(), Stop<E>> {
        let end_b = pb.checked_add(nb).ok_or_else(internal)?;
        let tmp: Vec<usize> = a.get(pb..end_b).ok_or_else(internal)?.to_vec();
        let inc = |x: usize| x.checked_add(1).ok_or_else(internal::<E>);
        let dec = |x: usize| x.checked_sub(1).ok_or_else(internal::<E>);
        let mut dest = end_b;
        let mut ia = pa.checked_add(na).ok_or_else(internal)?;
        let mut tb = nb;
        let exit = 'merge: {
            dest = dec(dest)?;
            ia = dec(ia)?;
            set(a, dest, get(a, ia).ok_or_else(internal)?).ok_or_else(internal)?;
            na = dec(na)?;
            if na == 0 {
                break 'merge Exit::Done;
            }
            if nb == 1 {
                break 'merge Exit::Copy;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut count_a = 0usize;
                let mut count_b = 0usize;
                loop {
                    let x = get(&tmp, dec(tb)?).ok_or_else(internal)?;
                    let y = get(a, dec(ia)?).ok_or_else(internal)?;
                    dest = dec(dest)?;
                    if (self.lt)(x, y)? {
                        ia = dec(ia)?;
                        set(a, dest, y).ok_or_else(internal)?;
                        count_a = inc(count_a)?;
                        count_b = 0;
                        na = dec(na)?;
                        if na == 0 {
                            break 'merge Exit::Done;
                        }
                        if count_a >= min_gallop {
                            break;
                        }
                    } else {
                        tb = dec(tb)?;
                        set(a, dest, x).ok_or_else(internal)?;
                        count_b = inc(count_b)?;
                        count_a = 0;
                        nb = dec(nb)?;
                        if nb == 1 {
                            break 'merge Exit::Copy;
                        }
                        if count_b >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop = inc(min_gallop)?;
                loop {
                    if min_gallop > 1 {
                        min_gallop = dec(min_gallop)?;
                    }
                    self.min_gallop = min_gallop;
                    let key = get(&tmp, dec(tb)?).ok_or_else(internal)?;
                    let k = self.gallop_right(key, a, pa, na, dec(na)?)?;
                    let k = na.checked_sub(k).ok_or_else(internal)?;
                    count_a = k;
                    if k > 0 {
                        for _ in 0..k {
                            dest = dec(dest)?;
                            ia = dec(ia)?;
                            set(a, dest, get(a, ia).ok_or_else(internal)?).ok_or_else(internal)?;
                        }
                        na = na.checked_sub(k).ok_or_else(internal)?;
                        if na == 0 {
                            break 'merge Exit::Done;
                        }
                    }
                    dest = dec(dest)?;
                    tb = dec(tb)?;
                    set(a, dest, get(&tmp, tb).ok_or_else(internal)?).ok_or_else(internal)?;
                    nb = dec(nb)?;
                    if nb == 1 {
                        break 'merge Exit::Copy;
                    }
                    let key = get(a, dec(ia)?).ok_or_else(internal)?;
                    let k = self.gallop_left(key, &tmp, 0, nb, dec(nb)?)?;
                    let k = nb.checked_sub(k).ok_or_else(internal)?;
                    count_b = k;
                    if k > 0 {
                        for _ in 0..k {
                            dest = dec(dest)?;
                            tb = dec(tb)?;
                            set(a, dest, get(&tmp, tb).ok_or_else(internal)?)
                                .ok_or_else(internal)?;
                        }
                        nb = nb.checked_sub(k).ok_or_else(internal)?;
                        if nb == 1 {
                            break 'merge Exit::Copy;
                        }
                        if nb == 0 {
                            break 'merge Exit::Done;
                        }
                    }
                    dest = dec(dest)?;
                    ia = dec(ia)?;
                    set(a, dest, get(a, ia).ok_or_else(internal)?).ok_or_else(internal)?;
                    na = dec(na)?;
                    if na == 0 {
                        break 'merge Exit::Done;
                    }
                    if count_a < MIN_GALLOP && count_b < MIN_GALLOP {
                        break;
                    }
                }
                min_gallop = inc(min_gallop)?;
                self.min_gallop = min_gallop;
            }
        };
        match exit {
            // `Succeed:` what is left of B (`tmp[..nb]`) goes just below `dest`.
            Exit::Done => {
                for _ in 0..nb {
                    dest = dec(dest)?;
                    tb = dec(tb)?;
                    set(a, dest, get(&tmp, tb).ok_or_else(internal)?).ok_or_else(internal)?;
                }
            }
            // `CopyA:` nb == 1: the rest of A moves up, B's single element goes before it.
            Exit::Copy => {
                for _ in 0..na {
                    dest = dec(dest)?;
                    ia = dec(ia)?;
                    set(a, dest, get(a, ia).ok_or_else(internal)?).ok_or_else(internal)?;
                }
                dest = dec(dest)?;
                set(a, dest, get(&tmp, dec(tb)?).ok_or_else(internal)?).ok_or_else(internal)?;
            }
        }
        Ok(())
    }

    /// `merge_at(i)`.
    fn merge_at(&mut self, a: &mut [usize], i: usize) -> Result<(), Stop<E>> {
        let ra = *self.pending.get(i).ok_or_else(internal)?;
        let rb = *self
            .pending
            .get(i.checked_add(1).ok_or_else(internal)?)
            .ok_or_else(internal)?;
        let (mut pa, mut na, pb, mut nb) = (ra.base, ra.len, rb.base, rb.len);
        if let Some(r) = self.pending.get_mut(i) {
            r.len = na.checked_add(nb).ok_or_else(internal)?;
        }
        // Drop run i+1 (sliding i+2 down when i is the third-last).
        let drop_at = i.checked_add(1).ok_or_else(internal)?;
        if drop_at < self.pending.len() {
            let rest: Vec<Run> = self
                .pending
                .iter()
                .skip(drop_at.saturating_add(1))
                .copied()
                .collect();
            self.pending.truncate(drop_at);
            self.pending.extend(rest);
        }
        let key = get(a, pb).ok_or_else(internal)?;
        let k = self.gallop_right(key, a, pa, na, 0)?;
        pa = pa.checked_add(k).ok_or_else(internal)?;
        na = na.checked_sub(k).ok_or_else(internal)?;
        if na == 0 {
            return Ok(());
        }
        let key = get(
            a,
            pa.checked_add(na)
                .and_then(|x| x.checked_sub(1))
                .ok_or_else(internal)?,
        )
        .ok_or_else(internal)?;
        nb = self.gallop_left(key, a, pb, nb, nb.checked_sub(1).ok_or_else(internal)?)?;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(a, pa, na, pb, nb)
        } else {
            self.merge_hi(a, pa, na, pb, nb)
        }
    }

    /// `found_new_run(n2)`: powersort merging of the pending runs.
    fn found_new_run(&mut self, a: &mut [usize], n2: usize, n: usize) -> Result<(), Stop<E>> {
        if let Some(top) = self.pending.last().copied() {
            let power = powerloop(top.base, top.len, n2, n).ok_or_else(internal)?;
            while self.pending.len() > 1 {
                let below = self.pending.len().checked_sub(2).ok_or_else(internal)?;
                if self.pending.get(below).ok_or_else(internal)?.power > power {
                    self.merge_at(a, below)?;
                } else {
                    break;
                }
            }
            if let Some(top) = self.pending.last_mut() {
                top.power = power;
            }
        }
        Ok(())
    }

    /// `merge_force_collapse`.
    fn force_collapse(&mut self, a: &mut [usize]) -> Result<(), Stop<E>> {
        while self.pending.len() > 1 {
            let mut i = self.pending.len().checked_sub(2).ok_or_else(internal)?;
            if i > 0 {
                let prev = self
                    .pending
                    .get(i.checked_sub(1).ok_or_else(internal)?)
                    .ok_or_else(internal)?
                    .len;
                let next = self
                    .pending
                    .get(i.checked_add(1).ok_or_else(internal)?)
                    .ok_or_else(internal)?
                    .len;
                if prev < next {
                    i = i.checked_sub(1).ok_or_else(internal)?;
                }
            }
            self.merge_at(a, i)?;
        }
        Ok(())
    }
}

/// `reverse_slice(a[lo..hi])`.
fn reverse(a: &mut [usize], lo: usize, hi: usize) -> Option<()> {
    a.get_mut(lo..hi)?.reverse();
    Some(())
}

/// `merge_compute_minrun(n)`.
fn compute_minrun(mut n: usize) -> usize {
    let mut r = 0usize;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n.saturating_add(r)
}

/// `powerloop(s1, n1, n2, n)`.
fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> Option<u32> {
    let mut result = 0u32;
    let mut a = s1.checked_mul(2)?.checked_add(n1)?;
    let mut b = a.checked_add(n1)?.checked_add(n2)?;
    loop {
        result = result.checked_add(1)?;
        if a >= n {
            a = a.checked_sub(n)?;
            b = b.checked_sub(n)?;
        } else if b >= n {
            break;
        }
        a = a.checked_shl(1)?;
        b = b.checked_shl(1)?;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<i64>, minor: PyMinor) -> Vec<i64> {
        let Ok(()) = sort(&mut v, minor, |a, b| Ok::<bool, !>(a < b));
        v
    }

    #[test]
    fn sorts_like_any_stable_sort_for_consistent_orders() {
        let mut x: u64 = 12345;
        for len in [0usize, 1, 2, 3, 10, 63, 64, 65, 100, 257, 1000, 5000] {
            let v: Vec<i64> = (0..len)
                .map(|_| {
                    x = x
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    i64::try_from(x >> 54).unwrap()
                })
                .collect();
            let mut want = v.clone();
            want.sort_unstable();
            assert_eq!(sorted(v.clone(), PyMinor::P312), want, "{len}");
            assert_eq!(sorted(v, PyMinor::P313), want, "{len}");
        }
        let desc: Vec<i64> = (0..300).rev().collect();
        assert_eq!(sorted(desc, PyMinor::P313), (0..300).collect::<Vec<_>>());
    }

    #[test]
    fn stability() {
        let mut v: Vec<(i32, usize)> = (0..500)
            .map(|i| (i32::try_from(i % 7).unwrap(), i))
            .collect();
        let Ok(()) = sort(&mut v, PyMinor::P312, |a, b| Ok::<bool, !>(a.0 < b.0));
        assert!(
            v.iter()
                .zip(v.iter().skip(1))
                .all(|(x, y)| x.0 < y.0 || (x.0 == y.0 && x.1 < y.1))
        );
    }

    #[test]
    fn raising_comparator_stops() {
        let mut v = vec![3, 1, 2];
        assert_eq!(
            sort(&mut v, PyMinor::P312, |_, _| Err::<bool, &str>("boom")),
            Err("boom")
        );
    }
}
