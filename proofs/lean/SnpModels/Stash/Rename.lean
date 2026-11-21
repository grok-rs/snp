import SnpModels.Stash.Protocol

/-!
# The restore by rename (K9, ADR-152)

ADR-152 restores the stash by renaming hard-linked originals back instead of running `git apply`, when a proof
holds. At model level the rename gives every file of the patch the user's original content back (`renameRestore`:
the preserved inode holds `W₀ p`) and leaves every other path as the hooks left it; the proof (`RenameProof`) is
what `DiffOracle::restore_proof` and the guard's checks establish about the worktree the hooks left: on every file
of the patch nothing converts (the clean and smudge filters are the identity there: ADR-119/133's conditions) and the
file is still what the stash's checkout wrote (every comparison since found no change).

`restoreR` is step 10 with the rename taken whenever the proof holds, `stashedRunR`/`runR` the run with it. The
theorems:

* `rename_restore_eq_apply`: under the proof, `git apply` succeeds (A1–A5, A2c) and its worktree is exactly the
  rename's;
* `runR_eq_run`: so the run with the rename ends exactly as the run without it (status, worktree, index, rollback
  flag, staged paths, kept patch), and every K9 theorem about `run` (T1–T6) holds for `runR` unchanged.

Not modelled (docs/VERIFICATION.md §9.2 K9 ¬G): that the preserved inode holds `W₀ p` (the guard's link-time check
`max(ctime, mtime) < M0` and its post-link stat comparison), the final stat and content check and the window between
it and the rename (a write landing there is overwritten: A4 excludes it here, ADR-152 records it), the metadata
(mode, owner, times), the journal, the partial-rename fallback and the backups' removal. The implementation is
stricter than `RenameProof` needs (it also refuses after a `stage_fixed` staging or a cleared intent-to-add entry,
and for a patch that deletes a file).
-/

namespace SnpModels.Stash

variable {P C : Type} [DecidableEq C]

/-- ADR-152: the restore by rename: every file of `S` gets the user's original back, every other path keeps what
the hooks left. -/
def renameRestore (W0 : Work P C) (S : PSet P) (W2 : Work P C) : Work P C :=
  fun p => if S p = true then W0 p else W2 p

/-- ADR-152's proof over the worktree `W2` the hooks left: on every file of the patch nothing converts and the file
is still what the stash's checkout wrote. -/
def RenameProof (g : Git P C) (I0 : Index P C) (W0 W2 : Work P C) : Prop :=
  ∀ p, dset g I0 W0 p = true →
    (∀ x, g.attrs.clean p x = x) ∧ (∀ x, g.attrs.smudge p x = x) ∧ W2 p = stashed g I0 W0 p

open Classical in
/-- Step 10 with ADR-152: the rename when its proof holds, otherwise `restore` (apply, rollback, apply). -/
noncomputable def restoreR (g : Git P C) (I0 : Index P C) (W0 : Work P C) (S : PSet P) (T : Tree P C)
    (W2 : Work P C) : Status × Work P C × Bool :=
  if RenameProof g I0 W0 W2 then (.ok, renameRestore W0 (dset g I0 W0) W2, false)
  else restore g (stashPatch g I0 W0) S T W2

/-- `stashedRun` with step 10 taken by `restoreR`. -/
noncomputable def stashedRunR (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C)
    (fixes : PSet P) : Outcome P C :=
  let Q := stashPatch g I0 W0
  let W2 := g.interfere 1 (hooks (stashWork g I0 W0))
  let st := stagedSet g Q W2 fixes
  let I2 := stage g.attrs st W2 (clearIta I0)
  let r := restoreR g I0 W0 (g.numstat Q) (treeOf I2) W2
  { status := r.1, work := r.2.1, index := readdIta I0 I2, rolledBack := r.2.2, staged := st,
    kept := if r.1 = .failed then some (Q, stashTree I0) else none }

open Classical in
/-- `run` with ADR-152's restore by rename. -/
noncomputable def runR (g : Git P C) (I0 : Index P C) (W0 : Work P C) (hooks : Work P C → Work P C)
    (fixes : PSet P) : Outcome P C :=
  if ∃ p, dom (stashPatch g I0 W0) p = true then
    if ∃ p, g.numstat (stashPatch g I0 W0) p = true then stashedRunR g I0 W0 hooks fixes
    else
      { status := .refused, work := W0, index := readdIta I0 (clearIta I0), rolledBack := false,
        staged := fun _ => false, kept := none }
  else
    let W2 := hooks W0
    let st : PSet P := if ∃ p, fixes p = true then fixes else fun _ => false
    { status := .nothing, work := W2, index := readdIta I0 (stage g.attrs st W2 (clearIta I0)),
      rolledBack := false, staged := st, kept := none }

/-- **ADR-152.** Under the rename's proof, `git apply` of the stashed patch onto the worktree the hooks left
succeeds, and its worktree is exactly the rename's. -/
theorem rename_restore_eq_apply {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g)
    {W0 W2 : Work P C} (hp : RenameProof g I0 W0 W2) :
    g.apply (stashPatch g I0 W0) W2 = some (renameRestore W0 (dset g I0 W0) W2) := by
  have hm : Matches g.attrs (stashPatch g I0 W0) W2 := by
    intro p q hq
    rw [stashPatch_eq] at hq
    obtain ⟨hv, hT, -⟩ := diffPatch_some hq
    have hd : dset g I0 W0 p = true := hv
    obtain ⟨-, -, hw⟩ := hp p hd
    rw [seen, hw]
    show (writeSet g.attrs (treeOf I0) (dset g I0 W0) W0 p).map (g.attrs.clean p) = some q.pre
    rw [writeSet_true hd hT, Option.map_some, a1_tree h p q.pre hT]
  rw [hc _ _ hm]
  congr 1
  funext p
  rw [stashPatch_eq, patched_diff]
  unfold renameRestore
  cases hv : visible g.attrs (treeOf I0) W0 p
  · have hd : dset g I0 W0 p = false := hv
    simp [hd]
  · have hd : dset g I0 W0 p = true := hv
    obtain ⟨hcl, hsm, -⟩ := hp p hd
    simp only [hd, ite_true, rt]
    cases W0 p with
    | none => rfl
    | some w => simp [hcl, hsm]

/-- Step 10 with the rename equals step 10 without it (A1–A5, A2c). -/
theorem restoreR_eq {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C)
    (S : PSet P) (T : Tree P C) (W2 : Work P C) :
    restoreR g I0 W0 S T W2 = restore g (stashPatch g I0 W0) S T W2 := by
  unfold restoreR
  split
  · next hp =>
    unfold restore
    rw [rename_restore_eq_apply h hc hp]
  · rfl

/-- **ADR-152.** The run with the restore by rename ends exactly as the run without it, so T1–T6 hold for it
unchanged (A1–A5, A2c). -/
theorem runR_eq_run {g : Git P C} {I0 : Index P C} (h : Hyps g I0) (hc : Complete g) (W0 : Work P C)
    (hooks : Work P C → Work P C) (fixes : PSet P) :
    runR g I0 W0 hooks fixes = run g I0 W0 hooks fixes := by
  have hs : stashedRunR g I0 W0 hooks fixes = stashedRun g I0 W0 hooks fixes := by
    simp only [stashedRunR, stashedRun, restoreR_eq h hc]
  unfold runR run
  rw [hs]

end SnpModels.Stash
