import SnpModels.Stash.Model

/-!
# Stash protocol theorems (K9)

Over the model of `SnpModels.Stash.Model`, under the named hypotheses `Hyps` (A1–A5) and, where stated,
`Complete` (A2c):

* T1 `roundtrip`: without hooks, every file outside the patch is as the user left it and every file of the patch is
  its clean-then-smudge round trip; `roundtrip_exact`: so without hidden changes in the patch's files, the worktree
  is restored exactly.
* T2 `residual_loss`: after a successful run, the paths whose content differs from the user's are exactly ADR-093's
  two residual classes (a file of the patch that also has a hidden change; a hidden change in a file a hook
  visibly modified before a conflict rollback) plus the hooks' own edits the protocol kept.
* T4 `rollback_complete`: the conflict rollback leaves no visible change, keeps every path outside its list as it
  was, agrees with pre-commit's whole-tree checkout on what git sees of every not-assume-unchanged path, and makes
  the patch apply.
* T5 `no_silent_loss`: a failing final apply is reported (`failed`), keeps the patch (which holds every visible
  change) and the stash-time tree, and the recovery the K+ hint prints restores what T1 restores.
* T6 `index_invariant`: the index ends as it started, except for the paths `stage_fixed` staged, which are among
  the requested ones.

The counterexample for pre-commit's `checkout -- .` (T3) is in `SnpModels.Stash.Counterexample`.
-/

namespace SnpModels.Stash

variable {P C : Type} [DecidableEq C]

/-! ## Derived states -/

/-- `D = dom Patch(W₀)`: the files of the stashed patch (the visible unstaged changes). -/
def dset (g : Git P C) (I0 : Index P C) (W0 : Work P C) : PSet P := visible g.attrs (treeOf I0) W0

/-- The worktree after the stash's checkout, under A3–A5: the patch's files from the index. -/
def stashed (g : Git P C) (I0 : Index P C) (W0 : Work P C) : Work P C :=
  writeSet g.attrs (treeOf I0) (dset g I0 W0) W0

/-- The worktree the hooks leave. -/
def hookWork (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C) : Work P C :=
  hooks (stashed g I0 W0)

/-- The rollback's paths: the patch's files and every file `git diff` lists after the hooks. -/
def rbSet (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C) : PSet P :=
  rollbackSet g.attrs (dset g I0 W0) (treeOf I0) (hookWork g I0 W0 hooks)

/-- The worktree after the conflict rollback's checkout. -/
def rollbackWork (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C) : Work P C :=
  writeSet g.attrs (treeOf I0) (rbSet g I0 W0 hooks) (hookWork g I0 W0 hooks)

/-! ## Step lemmas -/

theorem treeOf_clearIta (I : Index P C) : treeOf (clearIta I) = treeOf I := by
  funext p
  simp only [treeOf, clearIta]
  split
  · next h => rw [h]; rfl
  · rfl

theorem stashTree_eq (I : Index P C) : stashTree I = treeOf I := treeOf_clearIta I

omit [DecidableEq C] in
theorem stage_empty (a : Attrs P C) (W : Work P C) (I : Index P C) : stage a (fun _ => false) W I = I := by
  funext p
  simp [stage]

theorem visible_of_tree_none {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} (hT : T p = none) :
    visible a T W p = false := by
  simp [visible, hT]

theorem dom_diffPatch (a : Attrs P C) (T : Tree P C) (W : Work P C) (p : P) :
    dom (diffPatch a T W) p = visible a T W p := by
  unfold dom diffPatch
  cases hT : T p with
  | none => simp [visible_of_tree_none hT]
  | some x =>
    simp only
    cases hv : visible a T W p <;> simp

theorem diffPatch_some {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} {h : Hunk C}
    (hq : diffPatch a T W p = some h) : visible a T W p = true ∧ T p = some h.pre ∧ h.post = seen a W p := by
  unfold diffPatch at hq
  cases hT : T p with
  | none => simp [hT] at hq
  | some x =>
    simp only [hT] at hq
    cases hv : visible a T W p
    · simp [hv] at hq
    · simp only [hv, ite_true, Option.some.injEq] at hq
      subst hq
      exact ⟨rfl, rfl, rfl⟩

theorem patched_diff (a : Attrs P C) (T : Tree P C) (W0 W : Work P C) (p : P) :
    patched a (diffPatch a T W0) W p = if visible a T W0 p = true then rt a W0 p else W p := by
  unfold patched
  cases hq : diffPatch a T W0 p with
  | none =>
    have hd := dom_diffPatch a T W0 p
    simp only [dom, hq, Option.isSome_none] at hd
    simp [← hd]
  | some h =>
    obtain ⟨hv, -, hpost⟩ := diffPatch_some hq
    simp only [hv, ite_true, hpost, seen, rt, Option.map_map]
    rfl

omit [DecidableEq C] in
theorem writeSet_true {a : Attrs P C} {T : Tree P C} {S : PSet P} {W : Work P C} {p : P} {x : C}
    (hS : S p = true) (hT : T p = some x) : writeSet a T S W p = some (a.smudge p x) := by
  simp [writeSet, hS, hT]

omit [DecidableEq C] in
theorem writeSet_false {a : Attrs P C} {T : Tree P C} {S : PSet P} {W : Work P C} {p : P}
    (hS : S p = false) : writeSet a T S W p = W p := by
  simp [writeSet, hS]

omit [DecidableEq C] in
theorem writeSet_none {a : Attrs P C} {T : Tree P C} {S : PSet P} {W : Work P C} {p : P}
    (hT : T p = none) : writeSet a T S W p = W p := by
  unfold writeSet
  cases S p <;> simp [hT]

/-- A file git does not list, tracked and not assume-unchanged, is seen as its blob, so its round trip is the
smudged blob. -/
theorem seen_of_not_visible {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} {x : C}
    (hT : T p = some x) (hau : a.assumeUnchanged p = false) (hv : visible a T W p = false) :
    seen a W p = some x := by
  simp only [visible, hT, hau, Bool.not_false, Bool.true_and, decide_eq_false_iff_not, Decidable.not_not] at hv
  exact hv

theorem rt_of_not_visible {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} {x : C}
    (hT : T p = some x) (hau : a.assumeUnchanged p = false) (hv : visible a T W p = false) :
    rt a W p = some (a.smudge p x) := by
  have hs := seen_of_not_visible hT hau hv
  simp only [seen] at hs
  simp only [rt]
  cases hw : W p with
  | none => simp [hw] at hs
  | some w =>
    simp only [hw, Option.map_some, Option.some.injEq] at hs ⊢
    rw [hs]

theorem au_of_visible {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} (hv : visible a T W p = true) :
    a.assumeUnchanged p = false := by
  unfold visible at hv
  cases hT : T p with
  | none => simp [hT] at hv
  | some x =>
    simp only [hT, Bool.and_eq_true, Bool.not_eq_true'] at hv
    exact hv.1

theorem tree_of_visible {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} (hv : visible a T W p = true) :
    ∃ x, T p = some x := by
  unfold visible at hv
  cases hT : T p with
  | none => simp [hT] at hv
  | some x => exact ⟨x, rfl⟩

/-- A smudged blob is never visible (A1). -/
theorem not_visible_smudged {a : Attrs P C} {T : Tree P C} {W : Work P C} {p : P} {x : C}
    (h1 : a.clean p (a.smudge p x) = x) (hT : T p = some x) (hw : W p = some (a.smudge p x)) :
    visible a T W p = false := by
  simp [visible, hT, seen, hw, h1]

/-- After writing every file of the patch from the stash-time tree, every pre-image matches (A1). -/
theorem matches_writeSet {a : Attrs P C} {T : Tree P C} {S : PSet P} {W0 W : Work P C}
    (h1 : ∀ p x, T p = some x → a.clean p (a.smudge p x) = x)
    (hS : ∀ p, visible a T W0 p = true → S p = true) : Matches a (diffPatch a T W0) (writeSet a T S W) := by
  intro p h hq
  obtain ⟨hv, hT, -⟩ := diffPatch_some hq
  rw [seen, writeSet_true (hS p hv) hT, Option.map_some, h1 p h.pre hT]

/-! ## The run, unfolded -/

theorem stashPatch_eq (g : Git P C) (I0 : Index P C) (W0 : Work P C) :
    stashPatch g I0 W0 = diffPatch g.attrs (treeOf I0) W0 := by
  simp [stashPatch, stashTree_eq]

theorem numstat_eq {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (W0 : Work P C) :
    g.numstat (stashPatch g I0 W0) = dset g I0 W0 := by
  funext p
  rw [h.a5, stashPatch_eq, dom_diffPatch]
  rfl

theorem stashWork_eq {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (W0 : Work P C) :
    stashWork g I0 W0 = stashed g I0 W0 := by
  simp only [stashWork, h.a3, h.a4, numstat_eq h, stashTree_eq]
  rfl

omit [DecidableEq C] in
theorem a1_tree {g : Git P C} {I0 : Index P C} (h : Hyps g I0) :
    ∀ p x, treeOf I0 p = some x → g.attrs.clean p (g.attrs.smudge p x) = x := h.a1

theorem run_stashed {g : Git P C} {I0 : Index P C} (h : Hyps g I0) {W0 : Work P C}
    (hooks : Work P C → Work P C) (fixes : PSet P) (hD : ∃ p, dset g I0 W0 p = true) :
    run g I0 W0 hooks fixes = stashedRun g I0 W0 hooks fixes := by
  have h1 : ∃ p, dom (stashPatch g I0 W0) p = true := by
    obtain ⟨p, hp⟩ := hD
    exact ⟨p, by rw [stashPatch_eq, dom_diffPatch]; exact hp⟩
  have h2 : ∃ p, g.numstat (stashPatch g I0 W0) p = true := by
    rw [numstat_eq h]; exact hD
  unfold run
  rw [ite_eq_left h1, ite_eq_left h2]

theorem run_clean {g : Git P C} {I0 : Index P C} {W0 : Work P C}
    (hooks : Work P C → Work P C) (fixes : PSet P) (hD : ¬∃ p, dset g I0 W0 p = true) :
    (run g I0 W0 hooks fixes).status = .nothing ∧ (run g I0 W0 hooks fixes).work = hooks W0 ∧
      (run g I0 W0 hooks fixes).rolledBack = false ∧ (run g I0 W0 hooks fixes).kept = none := by
  have h1 : ¬∃ p, dom (stashPatch g I0 W0) p = true := by
    rintro ⟨p, hp⟩
    rw [stashPatch_eq, dom_diffPatch] at hp
    exact hD ⟨p, hp⟩
  unfold run
  rw [ite_eq_right h1]
  exact ⟨rfl, rfl, rfl, rfl⟩

/-- The three ways a stashed run ends: applied at once; rolled back and applied; rolled back and failed. -/
theorem stashedRun_cases {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (W0 : Work P C)
    (hooks : Work P C → Work P C) (fixes : PSet P) :
    (∃ W', g.apply (stashPatch g I0 W0) (hookWork g I0 W0 hooks) = some W' ∧
      (stashedRun g I0 W0 hooks fixes).status = .ok ∧
      (stashedRun g I0 W0 hooks fixes).work = patched g.attrs (stashPatch g I0 W0) (hookWork g I0 W0 hooks) ∧
      (stashedRun g I0 W0 hooks fixes).rolledBack = false ∧ (stashedRun g I0 W0 hooks fixes).kept = none) ∨
    (g.apply (stashPatch g I0 W0) (hookWork g I0 W0 hooks) = none ∧
      (stashedRun g I0 W0 hooks fixes).staged = (fun _ => false) ∧
      (stashedRun g I0 W0 hooks fixes).rolledBack = true ∧
      ((∃ W', g.apply (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) = some W' ∧
          (stashedRun g I0 W0 hooks fixes).status = .ok ∧
          (stashedRun g I0 W0 hooks fixes).work = patched g.attrs (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) ∧
          (stashedRun g I0 W0 hooks fixes).kept = none) ∨
        (g.apply (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) = none ∧
          (stashedRun g I0 W0 hooks fixes).status = .failed ∧
          (stashedRun g I0 W0 hooks fixes).work = rollbackWork g I0 W0 hooks ∧
          (stashedRun g I0 W0 hooks fixes).kept = some (stashPatch g I0 W0, stashTree I0)))) := by
  have hw2 : g.interfere 1 (hooks (stashWork g I0 W0)) = hookWork g I0 W0 hooks := by
    rw [h.a4, stashWork_eq h]; rfl
  cases ha : g.apply (stashPatch g I0 W0) (hookWork g I0 W0 hooks) with
  | some W' =>
    left
    have hpat := (h.a2 _ _ _ ha).2
    refine ⟨W', rfl, ?_, ?_, ?_, ?_⟩ <;> simp [stashedRun, restore, hw2, ha, hpat]
  | none =>
    right
    have hst : stagedSet g (stashPatch g I0 W0) (hookWork g I0 W0 hooks) fixes = fun _ => false := by
      unfold stagedSet
      rw [ite_eq_right]
      rintro ⟨-, hc⟩
      rw [h.a2check, ha] at hc
      simp at hc
    have hI2 : treeOf (stage g.attrs (fun _ => false) (hookWork g I0 W0 hooks) (clearIta I0)) = treeOf I0 := by
      rw [stage_empty, treeOf_clearIta]
    have hW3 : g.checkout (treeOf I0) (rollbackSet g.attrs (g.numstat (stashPatch g I0 W0)) (treeOf I0)
        (hookWork g I0 W0 hooks)) (hookWork g I0 W0 hooks) = rollbackWork g I0 W0 hooks := by
      rw [h.a3, numstat_eq h]; rfl
    refine ⟨rfl, ?_, ?_, ?_⟩
    · simp [stashedRun, hw2, hst]
    · simp only [stashedRun, restore, hw2, ha, hst, hI2, h.a4, hW3]
      split <;> rfl
    · cases hb : g.apply (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) with
      | some W'' =>
        left
        have hpat := (h.a2 _ _ _ hb).2
        refine ⟨W'', rfl, ?_, ?_, ?_⟩ <;> simp [stashedRun, restore, hw2, ha, hst, hI2, hW3, h.a4, hb, hpat]
      | none =>
        right
        refine ⟨rfl, ?_, ?_, ?_⟩ <;> simp [stashedRun, restore, hw2, ha, hst, hI2, hW3, h.a4, hb, stashTree_eq]

/-! ## T1: the round trip without hooks -/

/-- **T1.** Without hooks and without `stage_fixed`, the run succeeds (or has nothing to stash), every file outside
the patch is exactly as the user left it, and every file of the patch is its clean-then-smudge round trip. -/
theorem roundtrip {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C) :
    ((run g I0 W0 id (fun _ => false)).status = .ok ∨ (run g I0 W0 id (fun _ => false)).status = .nothing) ∧
    ∀ p, (run g I0 W0 id (fun _ => false)).work p =
      if dset g I0 W0 p = true then rt g.attrs W0 p else W0 p := by
  by_cases hD : ∃ p, dset g I0 W0 p = true
  · rw [run_stashed h id _ hD]
    have hm : Matches g.attrs (stashPatch g I0 W0) (hookWork g I0 W0 id) := by
      rw [stashPatch_eq]
      exact matches_writeSet (a1_tree h) (fun p hp => hp)
    have ha := hc _ _ hm
    rcases stashedRun_cases h W0 id (fun _ => false) with ⟨W', -, hs, hw, -, -⟩ | ⟨hn, -⟩
    · refine ⟨Or.inl hs, fun p => ?_⟩
      rw [hw, stashPatch_eq, patched_diff]
      cases hv : visible g.attrs (treeOf I0) W0 p
      · have hd : dset g I0 W0 p = false := hv
        simp only [hd, Bool.false_eq_true, ite_false]
        show writeSet g.attrs (treeOf I0) (dset g I0 W0) W0 p = W0 p
        exact writeSet_false hd
      · have hd : dset g I0 W0 p = true := hv
        simp [hd]
    · rw [ha] at hn; cases hn
  · obtain ⟨hs, hw, -, -⟩ := run_clean (g := g) (I0 := I0) (W0 := W0) id (fun _ => false) hD
    refine ⟨Or.inr hs, fun p => ?_⟩
    have hp : dset g I0 W0 p = false := by
      cases hq : dset g I0 W0 p
      · rfl
      · exact absurd ⟨p, hq⟩ hD
    rw [hw, hp]
    rfl

/-- **T1 (exact).** If no file of the patch also has a change git cannot see (in particular: no filters), the run
without hooks restores the user's worktree exactly. -/
theorem roundtrip_exact {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C)
    (hh : ∀ p, dset g I0 W0 p = true → ¬Hidden g.attrs W0 p) :
    (run g I0 W0 id (fun _ => false)).work = W0 := by
  funext p
  rw [(roundtrip h hc W0).2 p]
  split
  · next hp =>
    have := hh p hp
    simp only [Hidden, Decidable.not_not] at this
    exact this
  · rfl

/-! ## T2: the exact residual loss -/

/-- **T2.** After a successful run, a path's content differs from the user's exactly when

1. it is a file of the patch that also has a change git cannot see (ADR-093: "a file with both a visible and a
   hidden change still loses the hidden part"), or
2. the conflict rollback ran, a hook visibly modified the file, it is not in the patch, and it has a change git
   cannot see (ADR-093: "a hidden change in a file a hook visibly modifies before a rollback"), or
3. the protocol did not write it after the hooks and the hooks changed it (the hooks' own edit, kept as
   pre-commit keeps it).

No other path is ever changed: in particular an assume-unchanged file, an untracked file and a file whose only
change is hidden keep the user's content unless a hook edits them. Needs neither A1 nor A2c. -/
theorem residual_loss {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (W0 : Work P C)
    (hooks : Work P C → Work P C) (fixes : PSet P) (hok : (run g I0 W0 hooks fixes).status = .ok) (p : P) :
    (run g I0 W0 hooks fixes).work p ≠ W0 p ↔
      (dset g I0 W0 p = true ∧ Hidden g.attrs W0 p) ∨
      ((run g I0 W0 hooks fixes).rolledBack = true ∧ dset g I0 W0 p = false ∧
        visible g.attrs (treeOf I0) (hookWork g I0 W0 hooks) p = true ∧ Hidden g.attrs W0 p) ∨
      (dset g I0 W0 p = false ∧
        ¬((run g I0 W0 hooks fixes).rolledBack = true ∧
          visible g.attrs (treeOf I0) (hookWork g I0 W0 hooks) p = true) ∧
        hookWork g I0 W0 hooks p ≠ W0 p) := by
  by_cases hD : ∃ p, dset g I0 W0 p = true
  · rw [run_stashed h hooks fixes hD] at hok ⊢
    have hdp : ∀ (b : Bool), (dset g I0 W0 p = b) → True := fun _ _ => trivial
    clear hdp
    rcases stashedRun_cases h W0 hooks fixes with ⟨-, -, -, hw, hrb, -⟩ | ⟨-, -, hrb, ⟨-, -, -, hw, -⟩ | ⟨-, hs, -, -⟩⟩
    · rw [hw, hrb, stashPatch_eq, patched_diff]
      cases hp : dset g I0 W0 p
      · simp only [dset] at hp
        simp [hp, Hidden]
      · simp only [dset] at hp
        simp [hp, Hidden]
    · rw [hw, hrb, stashPatch_eq, patched_diff]
      cases hp : dset g I0 W0 p
      · simp only [dset] at hp
        simp only [hp, Bool.false_eq_true, ite_false, false_and, true_and, false_or, ne_eq]
        unfold rollbackWork
        cases hv : visible g.attrs (treeOf I0) (hookWork g I0 W0 hooks) p
        · have hR : rbSet g I0 W0 hooks p = false := by
            simp [rbSet, rollbackSet, dset, hp, hv]
          rw [writeSet_false hR]
          simp
        · obtain ⟨x, hT⟩ := tree_of_visible hv
          have hR : rbSet g I0 W0 hooks p = true := by
            simp [rbSet, rollbackSet, hv]
          rw [writeSet_true hR hT, Hidden, rt_of_not_visible hT (au_of_visible hv) hp]
          simp only [true_and, not_true_eq_false, false_and, or_false, ne_eq]
      · simp only [dset] at hp
        simp [hp, Hidden]
    · rw [hs] at hok; cases hok
  · obtain ⟨hs, hw, hrb, -⟩ := run_clean (g := g) (I0 := I0) (W0 := W0) hooks fixes hD
    have hp : dset g I0 W0 p = false := by
      cases hq : dset g I0 W0 p
      · rfl
      · exact absurd ⟨p, hq⟩ hD
    have hst : stashed g I0 W0 = W0 := by
      funext q
      have hq : dset g I0 W0 q = false := by
        cases hq : dset g I0 W0 q
        · rfl
        · exact absurd ⟨q, hq⟩ hD
      exact writeSet_false hq
    rw [hw, hrb, hp, hookWork, hst]
    simp

/-! ## T4: rollback completeness -/

/-- **T4.** When the conflict rollback runs: it leaves no change git can see, every path outside its list keeps
the hooks' content (so a change git cannot see stays), what git sees of every path that is not assume-unchanged is
what pre-commit's whole-tree `checkout -- .` would leave, every pre-image of the patch matches, and so (A2c) the
apply after it succeeds. -/
theorem rollback_complete {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C)
    (hooks : Work P C → Work P C) (fixes : PSet P) (hrb : (run g I0 W0 hooks fixes).rolledBack = true) :
    (run g I0 W0 hooks fixes).status = .ok ∧
    (run g I0 W0 hooks fixes).work = patched g.attrs (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) ∧
    (∀ p, visible g.attrs (treeOf I0) (rollbackWork g I0 W0 hooks) p = false) ∧
    (∀ p, rbSet g I0 W0 hooks p = false → rollbackWork g I0 W0 hooks p = hookWork g I0 W0 hooks p) ∧
    (∀ p, g.attrs.assumeUnchanged p = false →
      seen g.attrs (rollbackWork g I0 W0 hooks) p =
        seen g.attrs (writeSet g.attrs (treeOf I0) (tracked (treeOf I0)) (hookWork g I0 W0 hooks)) p) ∧
    Matches g.attrs (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) := by
  have hD : ∃ p, dset g I0 W0 p = true := by
    apply Classical.byContradiction
    intro hD
    rw [(run_clean (g := g) (I0 := I0) (W0 := W0) hooks fixes hD).2.2.1] at hrb
    cases hrb
  rw [run_stashed h hooks fixes hD] at hrb ⊢
  have hm : Matches g.attrs (stashPatch g I0 W0) (rollbackWork g I0 W0 hooks) := by
    rw [stashPatch_eq]
    exact matches_writeSet (a1_tree h) (fun p hp => by simp [rbSet, rollbackSet, dset, hp])
  have ha := hc _ _ hm
  have h1 := a1_tree h
  refine ⟨?_, ?_, ?_, ?_, ?_, hm⟩
  · rcases stashedRun_cases h W0 hooks fixes with ⟨-, -, -, -, hr, -⟩ | ⟨-, -, -, ⟨-, -, hs, -, -⟩ | ⟨hn, -⟩⟩
    · rw [hr] at hrb; cases hrb
    · exact hs
    · rw [ha] at hn; cases hn
  · rcases stashedRun_cases h W0 hooks fixes with ⟨-, -, -, -, hr, -⟩ | ⟨-, -, -, ⟨-, -, -, hw, -⟩ | ⟨hn, -⟩⟩
    · rw [hr] at hrb; cases hrb
    · exact hw
    · rw [ha] at hn; cases hn
  · intro p
    unfold rollbackWork
    cases hT : treeOf I0 p with
    | none => exact visible_of_tree_none hT
    | some x =>
      cases hR : rbSet g I0 W0 hooks p
      · rw [visible, hT]
        have hv : visible g.attrs (treeOf I0) (hookWork g I0 W0 hooks) p = false := by
          simp only [rbSet, rollbackSet, Bool.or_eq_false_iff] at hR
          exact hR.2
        rw [visible, hT] at hv
        simp only [seen, writeSet_false hR] at hv ⊢
        exact hv
      · exact not_visible_smudged (h1 p x hT) hT (writeSet_true hR hT)
  · intro p hR
    exact writeSet_false hR
  · intro p hau
    unfold rollbackWork
    simp only [seen]
    cases hT : treeOf I0 p with
    | none => rw [writeSet_none hT, writeSet_none hT]
    | some x =>
      have hpc : writeSet g.attrs (treeOf I0) (tracked (treeOf I0)) (hookWork g I0 W0 hooks) p =
          some (g.attrs.smudge p x) := writeSet_true (by simp [tracked, hT]) hT
      rw [hpc, Option.map_some, h1 p x hT]
      cases hR : rbSet g I0 W0 hooks p
      · rw [writeSet_false hR]
        have hv : visible g.attrs (treeOf I0) (hookWork g I0 W0 hooks) p = false := by
          simp only [rbSet, rollbackSet, Bool.or_eq_false_iff] at hR
          exact hR.2
        exact seen_of_not_visible (W := hookWork g I0 W0 hooks) hT hau hv
      · rw [writeSet_true hR hT, Option.map_some, h1 p x hT]

/-! ## T5: no silent loss when the final apply fails -/

/-- **T5.** A run whose final apply fails reports it (`failed`, never `ok`), has run the rollback, leaves the
rollback's worktree, keeps the patch and the stash-time tree, the kept patch holds what git saw of every changed
file, and (A2c) the recovery the K+ hint prints restores every file of the patch to its round trip, as T1 does. -/
theorem no_silent_loss {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (W0 : Work P C)
    (hooks : Work P C → Work P C) (fixes : PSet P) (hf : (run g I0 W0 hooks fixes).status = .failed) :
    (run g I0 W0 hooks fixes).kept = some (stashPatch g I0 W0, stashTree I0) ∧
    (run g I0 W0 hooks fixes).rolledBack = true ∧
    (run g I0 W0 hooks fixes).work = rollbackWork g I0 W0 hooks ∧
    (∀ p, dset g I0 W0 p = true → (stashPatch g I0 W0 p).map Hunk.post = some (seen g.attrs W0 p)) ∧
    (Complete g → recover g (stashPatch g I0 W0, stashTree I0) (run g I0 W0 hooks fixes).work =
      some (fun p => if dset g I0 W0 p = true then rt g.attrs W0 p else (run g I0 W0 hooks fixes).work p)) := by
  have hD : ∃ p, dset g I0 W0 p = true := by
    apply Classical.byContradiction
    intro hD
    rw [(run_clean (g := g) (I0 := I0) (W0 := W0) hooks fixes hD).1] at hf
    cases hf
  rw [run_stashed h hooks fixes hD] at hf ⊢
  have hcase : (stashedRun g I0 W0 hooks fixes).kept = some (stashPatch g I0 W0, stashTree I0) ∧
      (stashedRun g I0 W0 hooks fixes).rolledBack = true ∧
      (stashedRun g I0 W0 hooks fixes).work = rollbackWork g I0 W0 hooks := by
    rcases stashedRun_cases h W0 hooks fixes with ⟨-, -, hs, -⟩ | ⟨-, -, hr, ⟨-, -, hs, -⟩ | ⟨-, -, hw, hk⟩⟩
    · rw [hs] at hf; cases hf
    · rw [hs] at hf; cases hf
    · exact ⟨hk, hr, hw⟩
  refine ⟨hcase.1, hcase.2.1, hcase.2.2, ?_, ?_⟩
  · intro p hp
    rw [stashPatch_eq]
    cases hq : diffPatch g.attrs (treeOf I0) W0 p with
    | none =>
      have hd := dom_diffPatch g.attrs (treeOf I0) W0 p
      simp only [dom, hq, Option.isSome_none] at hd
      simp only [dset] at hp
      rw [hp] at hd; cases hd
    | some hk =>
      obtain ⟨-, -, hpost⟩ := diffPatch_some hq
      simp [hpost]
  · intro hc
    unfold recover
    simp only
    rw [h.a3, numstat_eq h, stashTree_eq]
    have hm : Matches g.attrs (stashPatch g I0 W0)
        (writeSet g.attrs (treeOf I0) (dset g I0 W0) (stashedRun g I0 W0 hooks fixes).work) := by
      rw [stashPatch_eq]
      exact matches_writeSet (a1_tree h) (fun p hp => hp)
    rw [hc _ _ hm]
    congr 1
    funext p
    rw [stashPatch_eq, patched_diff]
    cases hv : visible g.attrs (treeOf I0) W0 p
    · have hd : dset g I0 W0 p = false := hv
      simp only [hd, Bool.false_eq_true, ite_false]
      exact writeSet_false hd
    · have hd : dset g I0 W0 p = true := hv
      simp [hd]

/-! ## T6: the index -/

/-- **T6.** Whatever the hooks do and whatever the git oracles answer, the index ends as it started (the
intent-to-add entries cleared and re-added, nothing else touched), except for the paths `stage_fixed` staged, which
are among the requested ones. -/
theorem index_invariant (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C)
    (fixes : PSet P) (p : P) :
    ((run g I0 W0 hooks fixes).staged p = false → (run g I0 W0 hooks fixes).index p = I0 p) ∧
    ((run g I0 W0 hooks fixes).staged p = true → fixes p = true) := by
  have key : ∀ (st : PSet P) (W : Work P C), st p = false →
      readdIta I0 (stage g.attrs st W (clearIta I0)) p = I0 p := by
    intro st W hst
    simp only [readdIta, stage, hst, Bool.false_eq_true, ite_false, clearIta]
    by_cases hi : I0 p = some .ita
    · simp [hi]
    · simp [hi]
  have key0 : readdIta I0 (clearIta I0) p = I0 p := by
    have := key (fun _ => false) W0 rfl
    rwa [stage_empty] at this
  unfold run
  split
  · split
    · constructor
      · intro hst
        exact key _ _ hst
      · intro hst
        simp only [stashedRun, stagedSet] at hst
        split at hst
        · exact hst
        · cases hst
    · exact ⟨fun _ => key0, fun hst => by cases hst⟩
  · constructor
    · intro hst
      exact key _ _ hst
    · intro hst
      simp only at hst
      split at hst
      · exact hst
      · cases hst

end SnpModels.Stash
