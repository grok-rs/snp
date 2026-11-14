/-!
# Stash protocol model (K9): the state, git's operations and snp's protocol

A protocol model (docs/VERIFICATION.md §4.2 role 3, §9.2 K9) of `snp_stash::enter` + `StashGuard::stage_fixed` +
`StashGuard::restore` (`crates/snp-stash/src/guard.rs`) as it is after D-DATA-19 (ADR-093): the checkout after the
stash rewrites exactly the patch's files (`git apply --numstat -z`), the conflict rollback those plus every file
`git diff --name-only` lists, and a failing final apply keeps the patch and the stash-time tree (K+, D-DATA-01).

The model is generic in the path type `P` and the content type `C`:

* `Index P C`: each path has a blob (clean content, as git stores it), an intent-to-add entry, or nothing;
* `Work P C`: each path has a working-tree content or nothing (missing file);
* `Attrs`: the clean filter `c`, the smudge filter `s` (both per path, as `.gitattributes` selects them) and the
  `--assume-unchanged` set.

Git sees a working-tree file through its clean filter: a tracked path is *visible* (`git diff` lists it) when it is
not assume-unchanged and `c (W p) ≠ I p`. The stat cache is not modelled: content decides.

Git's operations are fields of `Git` (oracles), and what the theorems need of them are the named hypotheses of
`Hyps` (never `axiom` declarations): A1 `c ∘ s = id` on the index's blobs, A2 `git apply` is all-or-nothing and
`apply --check` is its dry run, A3 `git checkout` writes exactly the given set from the index, A4 no other writer
changes the worktree between the protocol's steps (`interfere` is the identity), A5 `git apply --numstat` lists
exactly the patch's files. `Complete` (A2c: an apply whose pre-images all match succeeds) is kept apart because the
loss characterisation (T2), the failure theorem (T5) and the index theorem (T6) do not need it.

What is not modelled: the lock, the journal and its durability, crashes and the `Drop` fallback, the re-verification
loop (A4 makes its first digest match), the two `git apply` attempts (one oracle call: `core.autocrlf` is not
modelled), a failing checkout, `diff-index` errors, the old-git refusal and the read-only store.
-/

namespace SnpModels.Stash

/-- A set of paths. -/
abbrev PSet (P : Type) := P → Bool

/-- The working tree: each path's content, or `none` (no file). -/
abbrev Work (P C : Type) := P → Option C

/-- A tree (as `git write-tree` records it): each path's blob, or `none`. -/
abbrev Tree (P C : Type) := P → Option C

/-- An index entry: a blob (clean content) or an intent-to-add entry (`git add -N`). -/
inductive Entry (C : Type) where
  | blob (x : C)
  | ita
  deriving DecidableEq

/-- The index. -/
abbrev Index (P C : Type) := P → Option (Entry C)

/-- The blob of an index entry. -/
def blobOf {C : Type} : Option (Entry C) → Option C
  | some (.blob x) => some x
  | _ => none

variable {P C : Type}

/-- `git write-tree` (intent-to-add entries carry no blob). -/
def treeOf (I : Index P C) : Tree P C := fun p => blobOf (I p)

/-- The repository's attributes: the clean and smudge filters, and the `--assume-unchanged` bits. -/
structure Attrs (P C : Type) where
  clean : P → C → C
  smudge : P → C → C
  assumeUnchanged : PSet P

/-- What git sees of a working-tree file: its content through the clean filter. -/
def seen (a : Attrs P C) (W : Work P C) (p : P) : Option C := (W p).map (a.clean p)

/-- The path's content after a clean-then-smudge round trip (what git can give back from what it saw). -/
def rt (a : Attrs P C) (W : Work P C) (p : P) : Option C :=
  (W p).map (fun w => a.smudge p (a.clean p w))

/-- The file has a change git cannot see: its content is not the smudge of its clean form. -/
def Hidden (a : Attrs P C) (W : Work P C) (p : P) : Prop := rt a W p ≠ W p

section
variable [DecidableEq C]

/-- `git diff` against `T` lists `p`: tracked, not assume-unchanged, and the clean content differs. -/
def visible (a : Attrs P C) (T : Tree P C) (W : Work P C) (p : P) : Bool :=
  match T p with
  | none => false
  | some x => !a.assumeUnchanged p && decide (seen a W p ≠ some x)

/-- One file of a patch: the pre-image blob and the post-image (clean content, `none` = deletion). -/
structure Hunk (C : Type) where
  pre : C
  post : Option C

/-- A patch: the files it changes. -/
abbrev Patch (P C : Type) := P → Option (Hunk C)

/-- `git diff-index --binary <tree>`: every visible file, from its blob to what git sees. -/
def diffPatch (a : Attrs P C) (T : Tree P C) (W : Work P C) : Patch P C := fun p =>
  match T p with
  | none => none
  | some x => if visible a T W p then some ⟨x, seen a W p⟩ else none

end

/-- The files of a patch. -/
def dom (Q : Patch P C) : PSet P := fun p => (Q p).isSome

/-- Every pre-image of `Q` is what git sees in `W` (the condition for `git apply` to succeed). -/
def Matches (a : Attrs P C) (Q : Patch P C) (W : Work P C) : Prop :=
  ∀ p h, Q p = some h → seen a W p = some h.pre

/-- The worktree after applying `Q` in full: post-images smudged, every other file unchanged. -/
def patched (a : Attrs P C) (Q : Patch P C) (W : Work P C) : Work P C := fun p =>
  match Q p with
  | some h => h.post.map (a.smudge p)
  | none => W p

/-- Writing the paths `S` from `T` (smudged); every other path, and every path `T` lacks, unchanged. -/
def writeSet (a : Attrs P C) (T : Tree P C) (S : PSet P) (W : Work P C) : Work P C := fun p =>
  match S p, T p with
  | true, some x => some (a.smudge p x)
  | _, _ => W p

/-- Every tracked path (what pre-commit's `git checkout -- .` may rewrite). -/
def tracked (T : Tree P C) : PSet P := fun p => (T p).isSome

/-- The git oracles. -/
structure Git (P C : Type) where
  attrs : Attrs P C
  /-- `git apply --whitespace=nowarn <patch>`: the new worktree, or `none` (refused). -/
  apply : Patch P C → Work P C → Option (Work P C)
  /-- `git apply --check`. -/
  check : Patch P C → Work P C → Bool
  /-- `git checkout` of a set of paths from the index (as a tree of blobs). -/
  checkout : Tree P C → PSet P → Work P C → Work P C
  /-- `git apply --numstat -z <patch>`, parsed (`Git::patch_paths`). -/
  numstat : Patch P C → PSet P
  /-- Any other writer between the protocol's steps (step number, worktree before → after). -/
  interfere : Nat → Work P C → Work P C

/-- A1: the smudge filter's output cleans back to the blob, for every blob of the index `I`. -/
def A1 (a : Attrs P C) (I : Index P C) : Prop :=
  ∀ p x, treeOf I p = some x → a.clean p (a.smudge p x) = x

/-- The named hypotheses of the model (docs/VERIFICATION.md §9.2 K9). -/
structure Hyps (g : Git P C) (I0 : Index P C) : Prop where
  /-- A1: `c ∘ s = id` on the image of the index. -/
  a1 : A1 g.attrs I0
  /-- A2: `git apply` is all-or-nothing: it succeeds only when every pre-image matches, and then writes the whole
  patch; a refusal changes nothing (the caller keeps its worktree). -/
  a2 : ∀ Q W W', g.apply Q W = some W' → Matches g.attrs Q W ∧ W' = patched g.attrs Q W
  /-- A2 (dry run): `git apply --check` says whether `git apply` would succeed on the same worktree. -/
  a2check : ∀ Q W, g.check Q W = (g.apply Q W).isSome
  /-- A3: `git checkout` writes exactly the given paths, from the index, and nothing else. -/
  a3 : ∀ T S W, g.checkout T S W = writeSet g.attrs T S W
  /-- A4: no concurrent writer. -/
  a4 : ∀ k W, g.interfere k W = W
  /-- A5: `git apply --numstat` lists exactly the patch's files (linked to K7's `parse_numstat_z`). -/
  a5 : ∀ Q p, g.numstat Q p = dom Q p

/-- A2c: an apply whose pre-images all match succeeds. -/
def Complete (g : Git P C) : Prop :=
  ∀ Q W, Matches g.attrs Q W → g.apply Q W = some (patched g.attrs Q W)

/-! ## The index operations -/

section
variable [DecidableEq C]

/-- `git rm --cached` of the intent-to-add entries (`_intent_to_add_cleared`). -/
def clearIta (I : Index P C) : Index P C := fun p => if I p = some .ita then none else I p

/-- `git add --intent-to-add` of the paths that had an intent-to-add entry in `I0` (a path already in the index is
left as it is). -/
def readdIta (I0 I : Index P C) : Index P C := fun p =>
  if I0 p = some .ita ∧ I p = none then some .ita else I p

end

/-- `git --literal-pathspecs add -- <paths>` (`stage_fixed`): each path's clean content, or removed if missing. -/
def stage (a : Attrs P C) (S : PSet P) (W : Work P C) (I : Index P C) : Index P C := fun p =>
  if S p then (W p).map (fun w => .blob (a.clean p w)) else I p

/-! ## The protocol -/

/-- How a run ended. -/
inductive Status where
  /-- No unstaged change: nothing stashed (hooks ran on the user's worktree). -/
  | nothing
  /-- Stashed and restored. -/
  | ok
  /-- Stashed; the final apply failed (K+: patch, tree and journal kept, the error reported). -/
  | failed
  /-- The patch lists no file: refused before any worktree mutation (`ValueError`). -/
  | refused
  deriving DecidableEq

/-- The end state of a run. -/
structure Outcome (P C : Type) where
  status : Status
  work : Work P C
  index : Index P C
  /-- The conflict rollback ran. -/
  rolledBack : Bool
  /-- The paths `stage_fixed` added to the index. -/
  staged : PSet P
  /-- K+: the patch and the stash-time tree, kept for the recovery hint. -/
  kept : Option (Patch P C × Tree P C)

section
variable [DecidableEq C]

/-- The rollback's paths (`Git::rollback_paths`): the patch's files and every file `git diff` lists. -/
def rollbackSet (a : Attrs P C) (S : PSet P) (T : Tree P C) (W : Work P C) : PSet P := fun p =>
  S p || visible a T W p

/-- Step 10 (`Inner::restore_patch`): apply; on refusal the rollback checkout and the apply again. Returns the
status, the worktree and whether the rollback ran. -/
def restore (g : Git P C) (Q : Patch P C) (S : PSet P) (T : Tree P C) (W2 : Work P C) :
    Status × Work P C × Bool :=
  match g.apply Q W2 with
  | some W' => (.ok, W', false)
  | none =>
    let W2' := g.interfere 2 W2
    let W3 := g.checkout T (rollbackSet g.attrs S T W2') W2'
    match g.apply Q (g.interfere 3 W3) with
    | some W' => (.ok, W', true)
    | none => (.failed, W3, true)

open Classical in
/-- The paths `stage_fixed` stages after a stash: the requested ones, when there are any and `git apply --check`
accepts the patch against the hook-fixed worktree; otherwise none (`Ok(false)`, or `Ok(true)` for no paths). -/
noncomputable def stagedSet (g : Git P C) (Q : Patch P C) (W2 : Work P C) (fixes : PSet P) : PSet P :=
  if (∃ p, fixes p = true) ∧ g.check Q W2 = true then fixes else fun _ => false

/-- The stash-time tree (`git write-tree` after the intent-to-add entries are cleared). -/
def stashTree (I0 : Index P C) : Tree P C := treeOf (clearIta I0)

/-- The stashed patch. -/
def stashPatch (g : Git P C) (I0 : Index P C) (W0 : Work P C) : Patch P C :=
  diffPatch g.attrs (stashTree I0) W0

/-- The worktree after the stash's checkout (step 8, `checkout_paths(patch_paths(patch))`). -/
def stashWork (g : Git P C) (I0 : Index P C) (W0 : Work P C) : Work P C :=
  g.checkout (stashTree I0) (g.numstat (stashPatch g I0 W0)) (g.interfere 0 W0)

/-- The run when a patch was stashed and its file list is not empty: hooks, `stage_fixed`, restore,
intent-to-add re-add. -/
noncomputable def stashedRun (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C)
    (fixes : PSet P) : Outcome P C :=
  let Q := stashPatch g I0 W0
  let W2 := g.interfere 1 (hooks (stashWork g I0 W0))
  let st := stagedSet g Q W2 fixes
  let I2 := stage g.attrs st W2 (clearIta I0)
  let r := restore g Q (g.numstat Q) (treeOf I2) W2
  { status := r.1, work := r.2.1, index := readdIta I0 I2, rolledBack := r.2.2, staged := st,
    kept := if r.1 = .failed then some (Q, stashTree I0) else none }

open Classical in
/-- `staged_files_only` as snp runs it (`enter`, the hooks, `stage_fixed`, `restore`). `hooks` is what the hooks
do to the worktree they are given; `fixes` the paths passed to `stage_fixed` (none: not called). -/
noncomputable def run (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C)
    (fixes : PSet P) : Outcome P C :=
  if ∃ p, dom (stashPatch g I0 W0) p = true then
    if ∃ p, g.numstat (stashPatch g I0 W0) p = true then stashedRun g I0 W0 hooks fixes
    else
      { status := .refused, work := W0, index := readdIta I0 (clearIta I0), rolledBack := false,
        staged := fun _ => false, kept := none }
  else
    let W2 := hooks W0
    let st : PSet P := if ∃ p, fixes p = true then fixes else fun _ => false
    { status := .nothing, work := W2, index := readdIta I0 (stage g.attrs st W2 (clearIta I0)),
      rolledBack := false, staged := st, kept := none }

/-- The recovery the K+ hint prints (`messages::k_plus_hint`): restore the patch's files from the stash-time tree
(`git restore --source=<tree> --worktree`), then `git apply` the kept patch. -/
def recover (g : Git P C) (k : Patch P C × Tree P C) (W : Work P C) : Option (Work P C) :=
  g.apply k.1 (g.checkout k.2 (g.numstat k.1) W)

/-! ## pre-commit's protocol (`staged_files_only.py`), for the counterexample -/

open Classical in
/-- pre-commit 4.7.0's `_unstaged_changes_cleared` (`staged_files_only.py:53-104`) with the whole-tree
`git -c submodule.recurse=0 checkout -- .` at the stash and in the rollback; no `stage_fixed`. -/
noncomputable def pcRun (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C) :
    Work P C :=
  let T := stashTree I0
  let Q := diffPatch g.attrs T W0
  if ∃ p, dom Q p = true then
    let W2 := hooks (g.checkout T (tracked T) W0)
    match g.apply Q W2 with
    | some W' => W'
    | none =>
      let W3 := g.checkout T (tracked T) W2
      match g.apply Q W3 with
      | some W' => W'
      | none => W3
  else hooks W0

end

end SnpModels.Stash
