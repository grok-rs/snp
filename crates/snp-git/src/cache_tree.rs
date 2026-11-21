//! git's cache-tree (the index's `TREE` extension), parsed strictly, and the tree objects `git write-tree` would
//! write for an index, built in git's `update_one` order (ADR-153). No spawn.
//!
//! The extension is a pre-order list of nodes, each `<name>\0<entry_count> <subtree_nr>\n` followed by the node's
//! object id when `entry_count >= 0` (an invalidated node has `-1` and no id), then its `subtree_nr` children
//! (`cache-tree.c` `write_one`/`read_one`). [`CacheTree::parse`] accepts exactly what git writes: an empty root
//! name; non-empty child names without `/`, in git's subtree order (shorter names first, then bytes) and so without
//! duplicates; `-1` or a canonical decimal `entry_count` and a canonical decimal `subtree_nr`, both within `int`;
//! and nothing left after the root. Anything else refuses ([`CacheTreeRefusal`]).
//!
//! `write-tree` prints the root's id without writing anything when the cache-tree is fully valid: every node has
//! `entry_count >= 0` and its object exists (`cache_tree_fully_valid`); it does not compare the cache-tree with the
//! index (a stale cache-tree is trusted). [`CacheTree::expected_objects`] is the stronger check `diff-index` needs:
//! the bytes of every node's tree object as `update_one` would build them from the index entries (a subdirectory
//! appears where its first entry appears, `%o` modes, `40000` and the child node's id for a subdirectory), with every
//! node's `entry_count` and child count matching the index. Comparing them with the stored objects proves that the
//! tree the cache-tree names is exactly the index's.
//!
//! Owner: WP-18.

use crate::index_file::{IndexEntry, IndexFile};

/// The deepest directory nesting [`CacheTree::expected_objects`] follows (it recurses once per level).
const MAX_DEPTH: usize = 256;

/// Why the cache-tree was not used (every refusal means "no proof").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CacheTreeRefusal {
    /// Not in the shape git writes (see the module documentation).
    Malformed,
    /// A node is invalidated (`entry_count < 0`) where a valid one is needed.
    Invalid,
    /// The index and the cache-tree disagree, or the index holds an entry no tree can (unmerged, intent-to-add, an
    /// odd mode, names out of order, an empty path component), or its object ids were not kept.
    Mismatch,
    /// Nested deeper than the builder follows, or a reservation failed.
    TooLarge,
}

/// One node: spans into the extension's bytes.
#[derive(Debug, Clone)]
struct Node {
    name: (usize, usize),
    entry_count: i32,
    /// The object id's start (valid nodes only).
    oid: Option<usize>,
    /// The children, as indices into [`CacheTree::nodes`], in file order.
    children: Vec<usize>,
}

/// A parsed cache-tree.
#[derive(Debug, Clone)]
pub struct CacheTree {
    bytes: Vec<u8>,
    hash_len: usize,
    /// Pre-order; the root is first.
    nodes: Vec<Node>,
}

/// A tree object as `write-tree` would write it: its id (from the cache-tree) and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeObject {
    /// The raw object id the cache-tree records for the node.
    pub oid: Vec<u8>,
    /// The tree object's body (without the `tree <size>\0` header).
    pub body: Vec<u8>,
}

impl CacheTree {
    /// Parses the `TREE` extension `ext` of an index with object ids of `hash_len` bytes.
    pub fn parse(ext: &[u8], hash_len: usize) -> Result<Self, CacheTreeRefusal> {
        if !matches!(hash_len, 20 | 32) {
            return Err(CacheTreeRefusal::Malformed);
        }
        let mut nodes: Vec<Node> = Vec::new();
        let mut pos = 0_usize;
        // (node index, children still to read, the previous child's name)
        let mut stack: Vec<(usize, usize, Option<&[u8]>)> = Vec::new();
        loop {
            let (node, subtree_nr) = read_node(ext, &mut pos, hash_len)?;
            let idx = nodes.len();
            let this = span(ext, node.name).ok_or(CacheTreeRefusal::Malformed)?;
            match stack.last_mut() {
                None if !this.is_empty() => return Err(CacheTreeRefusal::Malformed),
                None => {}
                Some(_) if this.is_empty() => return Err(CacheTreeRefusal::Malformed),
                Some((parent, left, prev)) => {
                    *left = left.checked_sub(1).ok_or(CacheTreeRefusal::Malformed)?;
                    // the subtree order: shorter names first, then bytes (strictly: no duplicates)
                    if prev.is_some_and(|p| (p.len(), p) >= (this.len(), this)) {
                        return Err(CacheTreeRefusal::Malformed);
                    }
                    *prev = Some(this);
                    let p = nodes.get_mut(*parent).ok_or(CacheTreeRefusal::Malformed)?;
                    p.children
                        .try_reserve(1)
                        .map_err(|_| CacheTreeRefusal::TooLarge)?;
                    p.children.push(idx);
                }
            }
            nodes
                .try_reserve(1)
                .map_err(|_| CacheTreeRefusal::TooLarge)?;
            nodes.push(node);
            stack
                .try_reserve(1)
                .map_err(|_| CacheTreeRefusal::TooLarge)?;
            stack.push((idx, subtree_nr, None));
            // pop every node whose children are all read
            while stack.last().is_some_and(|&(_, left, _)| left == 0) {
                stack.pop();
            }
            if stack.is_empty() {
                break;
            }
        }
        if pos != ext.len() {
            return Err(CacheTreeRefusal::Malformed);
        }
        let mut bytes: Vec<u8> = Vec::new();
        bytes
            .try_reserve_exact(ext.len())
            .map_err(|_| CacheTreeRefusal::TooLarge)?;
        bytes.extend_from_slice(ext);
        Ok(Self {
            bytes,
            hash_len,
            nodes,
        })
    }

    /// Every node is valid (`entry_count >= 0`): with every node's object present, `write-tree` writes nothing and
    /// prints [`Self::root_oid`].
    pub fn fully_valid(&self) -> bool {
        self.nodes.iter().all(|n| n.entry_count >= 0)
    }

    /// The root's object id, when the root is valid.
    pub fn root_oid(&self) -> Option<&[u8]> {
        self.nodes.first().and_then(|n| self.oid_of(n))
    }

    /// The object id of every valid node, in pre-order.
    pub fn node_oids(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.nodes.iter().filter_map(|n| self.oid_of(n))
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Never true: a parsed cache-tree has its root.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn oid_of(&self, n: &Node) -> Option<&[u8]> {
        let s = n.oid?;
        self.bytes.get(s..s.checked_add(self.hash_len)?)
    }

    fn name_of(&self, n: &Node) -> Option<&[u8]> {
        span(&self.bytes, n.name)
    }

    /// The tree object of every node, as `update_one` would build it from `index` (read with
    /// [`IndexFile::read_keep`], with the cache-tree's id length), children before their parent; refuses unless the
    /// cache-tree is fully valid and describes exactly the index's directories and entry counts, and every entry is
    /// merged, not intent-to-add, `100644`, `100755`, `120000` or `160000`, in strictly increasing name order.
    pub fn expected_objects(&self, index: &IndexFile) -> Result<Vec<TreeObject>, CacheTreeRefusal> {
        if index.hash_len() != self.hash_len || index.entries.is_empty() {
            return Err(CacheTreeRefusal::Mismatch);
        }
        let mut prev: Option<&[u8]> = None;
        for e in &index.entries {
            let name = index.name(e);
            if e.stage() != 0
                || e.intent_to_add()
                || !matches!(e.mode, 0o100_644 | 0o100_755 | 0o120_000 | 0o160_000)
                || index.oid(e).is_none()
                || prev.is_some_and(|p| p >= name)
            {
                return Err(CacheTreeRefusal::Mismatch);
            }
            prev = Some(name);
        }
        let mut out: Vec<TreeObject> = Vec::new();
        out.try_reserve(self.nodes.len())
            .map_err(|_| CacheTreeRefusal::TooLarge)?;
        let b = Builder {
            ct: self,
            index,
            entries: &index.entries,
        };
        b.node(0, 0, 0, index.entries.len(), 0, &mut out)?;
        if out.len() != self.nodes.len() {
            return Err(CacheTreeRefusal::Mismatch);
        }
        Ok(out)
    }
}

/// [`CacheTree::expected_objects`]' recursion.
struct Builder<'a> {
    ct: &'a CacheTree,
    index: &'a IndexFile,
    entries: &'a [IndexEntry],
}

impl Builder<'_> {
    fn name(&self, i: usize) -> Result<&[u8], CacheTreeRefusal> {
        self.entries
            .get(i)
            .map(|e| self.index.name(e))
            .ok_or(CacheTreeRefusal::Mismatch)
    }

    /// Node `n` covers the entries `lo..hi`, every one named `<base>...` with `base_len` bytes of base (ending in
    /// `/`, or empty at the root).
    fn node(
        &self,
        n: usize,
        base_len: usize,
        lo: usize,
        hi: usize,
        depth: usize,
        out: &mut Vec<TreeObject>,
    ) -> Result<(), CacheTreeRefusal> {
        if depth > MAX_DEPTH {
            return Err(CacheTreeRefusal::TooLarge);
        }
        let node = self.ct.nodes.get(n).ok_or(CacheTreeRefusal::Mismatch)?;
        let oid = self.ct.oid_of(node).ok_or(CacheTreeRefusal::Invalid)?;
        let count = usize::try_from(node.entry_count).map_err(|_| CacheTreeRefusal::Invalid)?;
        if count != hi.saturating_sub(lo) || lo >= hi {
            return Err(CacheTreeRefusal::Mismatch);
        }
        let mut body: Vec<u8> = Vec::new();
        let mut subs = 0_usize;
        let mut i = lo;
        while i < hi {
            let name = self.name(i)?;
            let rest = name.get(base_len..).ok_or(CacheTreeRefusal::Mismatch)?;
            let (mode, component, oid_bytes, next) = if let Some(k) = memchr::memchr(b'/', rest) {
                let component = rest.get(..k).ok_or(CacheTreeRefusal::Mismatch)?;
                let child_base = base_len
                    .checked_add(k)
                    .and_then(|v| v.checked_add(1))
                    .ok_or(CacheTreeRefusal::TooLarge)?;
                let prefix = name.get(..child_base).ok_or(CacheTreeRefusal::Mismatch)?;
                let mut j = i.saturating_add(1);
                while j < hi && self.name(j)?.starts_with(prefix) {
                    j = j.saturating_add(1);
                }
                let child = node
                    .children
                    .iter()
                    .copied()
                    .find(|&c| {
                        self.ct
                            .nodes
                            .get(c)
                            .and_then(|cn| self.ct.name_of(cn))
                            .is_some_and(|cname| cname == component)
                    })
                    .ok_or(CacheTreeRefusal::Mismatch)?;
                self.node(child, child_base, i, j, depth.saturating_add(1), out)?;
                let child_oid = self
                    .ct
                    .nodes
                    .get(child)
                    .and_then(|cn| self.ct.oid_of(cn))
                    .ok_or(CacheTreeRefusal::Invalid)?;
                subs = subs.saturating_add(1);
                (0o40_000_u32, component, child_oid, j)
            } else {
                let e = self.entries.get(i).ok_or(CacheTreeRefusal::Mismatch)?;
                let id = self.index.oid(e).ok_or(CacheTreeRefusal::Mismatch)?;
                (e.mode, rest, id, i.saturating_add(1))
            };
            if component.is_empty() {
                return Err(CacheTreeRefusal::Mismatch);
            }
            let octal = format!("{mode:o}");
            body.try_reserve(
                octal
                    .len()
                    .saturating_add(component.len())
                    .saturating_add(oid_bytes.len())
                    .saturating_add(2),
            )
            .map_err(|_| CacheTreeRefusal::TooLarge)?;
            body.extend_from_slice(octal.as_bytes());
            body.push(b' ');
            body.extend_from_slice(component);
            body.push(0);
            body.extend_from_slice(oid_bytes);
            i = next;
        }
        if subs != node.children.len() {
            return Err(CacheTreeRefusal::Mismatch);
        }
        let mut id: Vec<u8> = Vec::new();
        id.try_reserve_exact(oid.len())
            .map_err(|_| CacheTreeRefusal::TooLarge)?;
        id.extend_from_slice(oid);
        out.try_reserve(1).map_err(|_| CacheTreeRefusal::TooLarge)?;
        out.push(TreeObject { oid: id, body });
        Ok(())
    }
}

fn span(bytes: &[u8], (s, n): (usize, usize)) -> Option<&[u8]> {
    bytes.get(s..s.checked_add(n)?)
}

/// One node's header (and id) at `*pos`; returns the node (without children) and its `subtree_nr`.
fn read_node(
    ext: &[u8],
    pos: &mut usize,
    hash_len: usize,
) -> Result<(Node, usize), CacheTreeRefusal> {
    let rest = ext.get(*pos..).ok_or(CacheTreeRefusal::Malformed)?;
    let nul = memchr::memchr(0, rest).ok_or(CacheTreeRefusal::Malformed)?;
    let name_bytes = rest.get(..nul).ok_or(CacheTreeRefusal::Malformed)?;
    if name_bytes.contains(&b'/') {
        return Err(CacheTreeRefusal::Malformed);
    }
    let name = (*pos, nul);
    let mut at = pos
        .checked_add(nul)
        .and_then(|v| v.checked_add(1))
        .ok_or(CacheTreeRefusal::Malformed)?;
    let entry_count = if ext.get(at) == Some(&b'-') {
        at = at.checked_add(1).ok_or(CacheTreeRefusal::Malformed)?;
        if number(ext, &mut at)? != 1 {
            return Err(CacheTreeRefusal::Malformed);
        }
        -1
    } else {
        number(ext, &mut at)?
    };
    if ext.get(at) != Some(&b' ') {
        return Err(CacheTreeRefusal::Malformed);
    }
    at = at.checked_add(1).ok_or(CacheTreeRefusal::Malformed)?;
    let subtree_nr = number(ext, &mut at)?;
    if ext.get(at) != Some(&b'\n') {
        return Err(CacheTreeRefusal::Malformed);
    }
    at = at.checked_add(1).ok_or(CacheTreeRefusal::Malformed)?;
    let oid = if entry_count >= 0 {
        let s = at;
        at = at
            .checked_add(hash_len)
            .filter(|e| *e <= ext.len())
            .ok_or(CacheTreeRefusal::Malformed)?;
        Some(s)
    } else {
        None
    };
    // every child takes at least a name byte, a NUL and "-1 0\n"
    let subtree_nr = usize::try_from(subtree_nr).map_err(|_| CacheTreeRefusal::Malformed)?;
    if subtree_nr > ext.len().saturating_sub(at) / 7 {
        return Err(CacheTreeRefusal::Malformed);
    }
    *pos = at;
    Ok((
        Node {
            name,
            entry_count,
            oid,
            children: Vec::new(),
        },
        subtree_nr,
    ))
}

/// A canonical non-negative decimal (no sign, no leading zero) within `int`, at `*at`.
fn number(ext: &[u8], at: &mut usize) -> Result<i32, CacheTreeRefusal> {
    let digits = ext
        .get(*at..)
        .map_or(0, |r| r.iter().take_while(|b| b.is_ascii_digit()).count());
    let d = ext
        .get(*at..at.saturating_add(digits))
        .ok_or(CacheTreeRefusal::Malformed)?;
    if d.is_empty() || (d.len() > 1 && d.first() == Some(&b'0')) {
        return Err(CacheTreeRefusal::Malformed);
    }
    let mut v: i32 = 0;
    for &c in d {
        v = v
            .checked_mul(10)
            .and_then(|x| x.checked_add(i32::from(c.wrapping_sub(b'0'))))
            .ok_or(CacheTreeRefusal::Malformed)?;
    }
    *at = at.saturating_add(digits);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node image.
    fn node(name: &[u8], count: i32, subs: usize, oid: Option<[u8; 20]>) -> Vec<u8> {
        let mut b = name.to_vec();
        b.push(0);
        b.extend_from_slice(format!("{count} {subs}\n").as_bytes());
        if let Some(o) = oid {
            b.extend_from_slice(&o);
        }
        b
    }

    #[test]
    fn parses_what_git_writes() {
        let ext = [
            node(b"", 3, 2, Some([1; 20])),
            node(b"b", 1, 0, Some([2; 20])),
            node(b"aa", 1, 1, Some([3; 20])),
            node(b"x", -1, 0, None),
        ]
        .concat();
        let ct = CacheTree::parse(&ext, 20).unwrap();
        assert_eq!(ct.len(), 4);
        assert!(!ct.fully_valid());
        assert_eq!(ct.root_oid(), Some(&[1; 20][..]));
        assert_eq!(ct.node_oids().count(), 3);
        let valid = [node(b"", 1, 0, Some([1; 20]))].concat();
        assert!(CacheTree::parse(&valid, 20).unwrap().fully_valid());
    }

    #[test]
    fn refuses_anything_else() {
        let ok = node(b"", 1, 0, Some([1; 20]));
        let bad: Vec<Vec<u8>> = vec![
            // a named root, leftovers, a missing id, a missing child
            node(b"r", 1, 0, Some([1; 20])),
            [ok.clone(), vec![0]].concat(),
            ok[..ok.len() - 1].to_vec(),
            node(b"", 1, 1, Some([1; 20])),
            // children out of git's order or duplicated, an empty or slashed child name
            [
                node(b"", 2, 2, Some([1; 20])),
                node(b"bb", 1, 0, Some([2; 20])),
                node(b"a", 1, 0, Some([3; 20])),
            ]
            .concat(),
            [
                node(b"", 2, 2, Some([1; 20])),
                node(b"a", 1, 0, Some([2; 20])),
                node(b"a", 1, 0, Some([3; 20])),
            ]
            .concat(),
            [
                node(b"", 1, 1, Some([1; 20])),
                node(b"", 1, 0, Some([2; 20])),
            ]
            .concat(),
            [
                node(b"", 1, 1, Some([1; 20])),
                node(b"a/b", 1, 0, Some([2; 20])),
            ]
            .concat(),
            // numbers git does not write
            b"\x00-2 0\n".to_vec(),
            [b"\x0001 0\n".to_vec(), vec![1; 20]].concat(),
            [b"\x00+1 0\n".to_vec(), vec![1; 20]].concat(),
            [b"\x001  0\n".to_vec(), vec![1; 20]].concat(),
            [b"\x002147483648 0\n".to_vec(), vec![1; 20]].concat(),
            b"".to_vec(),
        ];
        for b in bad {
            assert_eq!(
                CacheTree::parse(&b, 20).unwrap_err(),
                CacheTreeRefusal::Malformed,
                "{b:?}"
            );
        }
        assert!(CacheTree::parse(&ok, 21).is_err());
    }

    /// An index image with these (name, mode, oid byte) entries and the given TREE extension.
    fn index(entries: &[(&[u8], u32, u8)], tree: &[u8]) -> IndexFile {
        let mut b = b"DIRC".to_vec();
        b.extend_from_slice(&2_u32.to_be_bytes());
        b.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        for (name, mode, o) in entries {
            let mut e = Vec::new();
            for w in [0_u32, 0, 0, 0, 0, 0, *mode, 0, 0, 1] {
                e.extend_from_slice(&w.to_be_bytes());
            }
            e.extend_from_slice(&[*o; 20]);
            e.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
            e.extend_from_slice(name);
            let padded = (e.len() + 8) & !7;
            e.resize(padded, 0);
            b.extend_from_slice(&e);
        }
        b.extend_from_slice(b"TREE");
        b.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_be_bytes());
        b.extend_from_slice(tree);
        b.extend_from_slice(&[0; 20]);
        IndexFile::parse_keep(&b, 20).unwrap()
    }

    fn entry(mode: &str, name: &[u8], oid: &[u8]) -> Vec<u8> {
        [mode.as_bytes(), b" ", name, b"\0", oid].concat()
    }

    #[test]
    fn expected_objects_follow_update_one() {
        // `a.txt` sorts before `a/` in the index and in the tree; `b` is a symlink, `c` a gitlink
        let tree = [
            node(b"", 4, 1, Some([9; 20])),
            node(b"a", 2, 1, Some([8; 20])),
            node(b"d", 1, 0, Some([7; 20])),
        ]
        .concat();
        let ix = index(
            &[
                (b"a.txt", 0o100_644, 1),
                (b"a/d/x", 0o100_755, 2),
                (b"a/y", 0o100_644, 3),
                (b"b", 0o120_000, 4),
            ],
            &tree,
        );
        let ct = CacheTree::parse(ix.tree_extension().unwrap(), 20).unwrap();
        let objs = ct.expected_objects(&ix).unwrap();
        assert_eq!(objs.len(), 3);
        assert_eq!(objs[0].oid, [7; 20]);
        assert_eq!(objs[0].body, entry("100755", b"x", &[2; 20]));
        assert_eq!(objs[1].oid, [8; 20]);
        assert_eq!(
            objs[1].body,
            [
                entry("40000", b"d", &[7; 20]),
                entry("100644", b"y", &[3; 20])
            ]
            .concat()
        );
        assert_eq!(objs[2].oid, [9; 20]);
        assert_eq!(
            objs[2].body,
            [
                entry("100644", b"a.txt", &[1; 20]),
                entry("40000", b"a", &[8; 20]),
                entry("120000", b"b", &[4; 20]),
            ]
            .concat()
        );
    }

    #[test]
    fn expected_objects_refuse_a_cache_tree_that_disagrees() {
        let ents: &[(&[u8], u32, u8)] = &[(b"a/x", 0o100_644, 1), (b"b", 0o100_644, 2)];
        let cases: Vec<(Vec<u8>, CacheTreeRefusal)> = vec![
            // a wrong root count, a wrong child count, a missing child node, an extra child node, an invalid child
            (
                [
                    node(b"", 3, 1, Some([9; 20])),
                    node(b"a", 1, 0, Some([8; 20])),
                ]
                .concat(),
                CacheTreeRefusal::Mismatch,
            ),
            (
                [
                    node(b"", 2, 1, Some([9; 20])),
                    node(b"a", 2, 0, Some([8; 20])),
                ]
                .concat(),
                CacheTreeRefusal::Mismatch,
            ),
            (node(b"", 2, 0, Some([9; 20])), CacheTreeRefusal::Mismatch),
            (
                [
                    node(b"", 2, 2, Some([9; 20])),
                    node(b"a", 1, 0, Some([8; 20])),
                    node(b"zz", 1, 0, Some([7; 20])),
                ]
                .concat(),
                CacheTreeRefusal::Mismatch,
            ),
            (
                [node(b"", 2, 1, Some([9; 20])), node(b"a", -1, 0, None)].concat(),
                CacheTreeRefusal::Invalid,
            ),
            (
                [node(b"", -1, 1, None), node(b"a", 1, 0, Some([8; 20]))].concat(),
                CacheTreeRefusal::Invalid,
            ),
        ];
        for (tree, want) in cases {
            let ix = index(ents, &tree);
            let ct = CacheTree::parse(ix.tree_extension().unwrap(), 20).unwrap();
            assert_eq!(ct.expected_objects(&ix).unwrap_err(), want, "{tree:?}");
        }
        // an odd mode, and an index read without its ids
        let tree = node(b"", 1, 0, Some([9; 20]));
        let ix = index(&[(b"a", 0o100_664, 1)], &tree);
        let ct = CacheTree::parse(ix.tree_extension().unwrap(), 20).unwrap();
        assert_eq!(
            ct.expected_objects(&ix).unwrap_err(),
            CacheTreeRefusal::Mismatch
        );
        let plain = IndexFile::default();
        assert_eq!(
            ct.expected_objects(&plain).unwrap_err(),
            CacheTreeRefusal::Mismatch
        );
    }
}
