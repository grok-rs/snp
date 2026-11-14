import SnpModels.Stash.Protocol

/-!
# T3: pre-commit's `checkout -- .` loses a change git cannot see (K9)

A machine-checked counterexample for pre-commit 4.7.0's protocol (`pcRun`, the whole-tree checkout), with the same
git (`idealGit`, which satisfies every hypothesis A1–A5 and A2c) on which snp's protocol keeps the change.

The two cases are D-DATA-19's two kinds of hidden change, the witnesses of `crates/snp-stash/tests/behaviour/
hidden.rs`:

* `filterCx` (`a_change_a_clean_filter_hides_survives_the_stash`): path 0 is `secret.cfg` under a clean filter that
  drops the hidden part (content `n` is seen as `n % 10`; `21` is "key=1" plus a `SECRET` line), path 1 is `foo`
  with an ordinary unstaged change that triggers the stash;
* `auCx` (`an_assume_unchanged_edit_survives_the_stash`): path 0 is an `--assume-unchanged` file with an edit.
-/

namespace SnpModels.Stash

variable {P C : Type} [DecidableEq C]

open Classical in
/-- A git that behaves exactly as the hypotheses say: apply succeeds iff every pre-image matches. -/
noncomputable def idealGit (a : Attrs P C) : Git P C where
  attrs := a
  apply Q W := if Matches a Q W then some (patched a Q W) else none
  check Q W := (if Matches a Q W then some (patched a Q W) else none).isSome
  checkout T S W := writeSet a T S W
  numstat Q := dom Q
  interfere _ W := W

omit [DecidableEq C] in
theorem idealGit_hyps {a : Attrs P C} {I0 : Index P C} (h1 : A1 a I0) : Hyps (idealGit a) I0 where
  a1 := h1
  a2 Q W W' h := by
    simp only [idealGit] at h
    split at h
    · next hm => exact ⟨hm, (Option.some.inj h).symm⟩
    · cases h
  a2check _ _ := rfl
  a3 _ _ _ := rfl
  a4 _ _ := rfl
  a5 _ _ := rfl

omit [DecidableEq C] in
theorem idealGit_complete (a : Attrs P C) : Complete (idealGit a) := by
  intro Q W hm
  exact ite_eq_left hm

/-- pre-commit's run without hooks, when there is something to stash: the patch's files are their round trip and
every other tracked file is rewritten from the index. -/
theorem pc_roundtrip {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C)
    (hD : ∃ p, dset g I0 W0 p = true) (p : P) :
    pcRun g I0 W0 id p =
      if dset g I0 W0 p = true then rt g.attrs W0 p else writeSet g.attrs (treeOf I0) (tracked (treeOf I0)) W0 p := by
  have h1 : ∃ p, dom (diffPatch g.attrs (treeOf I0) W0) p = true := by
    obtain ⟨q, hq⟩ := hD
    exact ⟨q, by rw [dom_diffPatch]; exact hq⟩
  have hm : Matches g.attrs (diffPatch g.attrs (treeOf I0) W0)
      (writeSet g.attrs (treeOf I0) (tracked (treeOf I0)) W0) := by
    refine matches_writeSet (a1_tree h) (fun q hq => ?_)
    obtain ⟨x, hT⟩ := tree_of_visible hq
    simp [tracked, hT]
  have h1' : ∃ p, dom (diffPatch g.attrs (stashTree I0) W0) p = true := by rw [stashTree_eq]; exact h1
  unfold pcRun
  rw [ite_eq_left h1']
  simp only [stashTree_eq, id]
  rw [h.a3, hc _ _ hm]
  simp only
  rw [patched_diff]
  rfl

/-! ## A change a clean filter hides -/

/-- `secret.cfg` (path 0) under `filter.strip.clean`: content `n` is seen as `n % 10`; no smudge. -/
def filterAttrs : Attrs Nat Nat where
  clean _ x := x % 10
  smudge _ x := x
  assumeUnchanged _ := false

/-- Both files are committed with content 1. -/
def cxIndex : Index Nat Nat := fun p => if p = 0 ∨ p = 1 then some (.blob 1) else none

/-- `secret.cfg` gained a hidden line (21, seen as 1); `foo` has a visible change (2). -/
def filterWork : Work Nat Nat := fun p => if p = 0 then some 21 else if p = 1 then some 2 else none

theorem cxIndex_a1 (a : Attrs Nat Nat) (ha : ∀ p, a.clean p (a.smudge p 1) = 1) : A1 a cxIndex := by
  intro p x hx
  simp only [treeOf, cxIndex] at hx
  split at hx
  · simp only [blobOf, Option.some.injEq] at hx
    subst hx
    exact ha p
  · simp [blobOf] at hx

/-- **T3 (clean filter).** On the same git (every hypothesis holds), pre-commit's run leaves `secret.cfg` as the
committed `1`: the hidden line is lost; snp's run gives back `21`. -/
theorem precommit_loses_filter_hidden_change :
    Hyps (idealGit filterAttrs) cxIndex ∧ Complete (idealGit filterAttrs) ∧
    filterWork 0 = some 21 ∧
    pcRun (idealGit filterAttrs) cxIndex filterWork id 0 = some 1 ∧
    (run (idealGit filterAttrs) cxIndex filterWork id (fun _ => false)).work 0 = some 21 := by
  have h := idealGit_hyps (cxIndex_a1 filterAttrs (fun _ => rfl))
  have hc := idealGit_complete filterAttrs
  have hD : ∃ p, dset (idealGit filterAttrs) cxIndex filterWork p = true := ⟨1, by decide⟩
  have h0 : dset (idealGit filterAttrs) cxIndex filterWork 0 = false := by decide
  refine ⟨h, hc, rfl, ?_, ?_⟩
  · rw [pc_roundtrip h hc filterWork hD 0, h0]
    rfl
  · rw [(roundtrip h hc filterWork).2 0, h0]
    rfl

/-! ## An edit in an `--assume-unchanged` file -/

/-- Path 0 is `--assume-unchanged`; no filters. -/
def auAttrs : Attrs Nat Nat where
  clean _ x := x
  smudge _ x := x
  assumeUnchanged p := p == 0

/-- `local.cfg` (path 0) has a machine-local edit (5); `foo` has a visible change (2). -/
def auWork : Work Nat Nat := fun p => if p = 0 then some 5 else if p = 1 then some 2 else none

/-- **T3 (assume-unchanged).** pre-commit's run rewrites the assume-unchanged file from the index (`1`); snp's
run keeps the edit (`5`). -/
theorem precommit_loses_assume_unchanged_edit :
    Hyps (idealGit auAttrs) cxIndex ∧ Complete (idealGit auAttrs) ∧
    auWork 0 = some 5 ∧
    pcRun (idealGit auAttrs) cxIndex auWork id 0 = some 1 ∧
    (run (idealGit auAttrs) cxIndex auWork id (fun _ => false)).work 0 = some 5 := by
  have h := idealGit_hyps (cxIndex_a1 auAttrs (fun _ => rfl))
  have hc := idealGit_complete auAttrs
  have hD : ∃ p, dset (idealGit auAttrs) cxIndex auWork p = true := ⟨1, by decide⟩
  have h0 : dset (idealGit auAttrs) cxIndex auWork 0 = false := by decide
  refine ⟨h, hc, rfl, ?_, ?_⟩
  · rw [pc_roundtrip h hc auWork hD 0, h0]
    rfl
  · rw [(roundtrip h hc auWork).2 0, h0]
    rfl

end SnpModels.Stash
