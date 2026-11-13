/-!
# `end-of-file-fixer` reference model

Three descriptions of pre-commit-hooks' `end_of_file_fixer.fix_file` (`end_of_file_fixer.py:9-49`):

* `pyFixFile`  – a line-by-line transliteration of the Python (seek/read/tell loop), the *reference*;
* `fix`        – a declarative spec: split the file into body + trailing CR/LF run and rewrite the run;
* `kernel`     – a transliteration of snp's Rust kernel (`snp_hooks::pch::p0::kernels::end_of_file_run`).

Proved below: all three agree, the fix is idempotent, it never touches the body, and the return code is 1
exactly when the content changes.
-/

namespace SnpModels.Eof

/-- `b'\n'` or `b'\r'`. -/
@[inline] def isEol (b : UInt8) : Bool := b == 10 || b == 13

/-! ## Reference: literal transliteration of the Python -/

/-- `fix_file(file_obj)` on a file whose bytes are `f`. Returns the new bytes and the return value.
Positions are byte offsets; `f.getD i 0` is "seek to `i`, `read(1)`". -/
def pyFixFile (f : List UInt8) : List UInt8 × Nat :=
  if f.length = 0 then (f, 0)                               -- seek(-1, SEEK_END) raises OSError: return 0
  else if !isEol (f.getD (f.length - 1) 0) then             -- last_character not in {\n, \r}
    (f ++ [10], 1)                                          -- seek(0, SEEK_END); write(b'\n'); return 1
  else loop (f.length - 1)
where
  /-- `while last_character in {b'\n', b'\r'}` where the byte at offset `i` was just read (`tell() == i+1`). -/
  loop : Nat → List UInt8 × Nat
    | 0 => ([], 1)                                          -- tell() == 1: seek(0); truncate(); return 1
    | i + 1 =>
      if isEol (f.getD i 0) then loop i                     -- seek(-2, SEEK_CUR); read(1)
      else trailer (i + 1)
  /-- After the loop: `position = tell()`, `remaining = read()`. -/
  trailer (position : Nat) : List UInt8 × Nat :=
    let remaining := f.drop position
    if remaining == [10] then (f, 0)                        -- for sequence in (b'\n', b'\r\n', b'\r'):
    else if [10].isPrefixOf remaining then (f.take (position + 1), 1)
    else if remaining == [13, 10] then (f, 0)
    else if [13, 10].isPrefixOf remaining then (f.take (position + 2), 1)
    else if remaining == [13] then (f, 0)
    else if [13].isPrefixOf remaining then (f.take (position + 1), 1)
    else (f, 0)

/-! ## Declarative spec -/

/-- `(body, run)`: `run` is the maximal trailing run of CR/LF bytes, `body ++ run = f`. -/
def splitTail : List UInt8 → List UInt8 × List UInt8
  | [] => ([], [])
  | b :: bs =>
    let p := splitTail bs
    if p.1.isEmpty && isEol b then ([], b :: p.2) else (b :: p.1, p.2)

/-- The spec: keep the body and exactly one line terminator; an all-CR/LF file becomes empty; a file without a
terminator gets `\n`. -/
def fix (f : List UInt8) : List UInt8 :=
  let p := splitTail f
  if f.isEmpty then f
  else if p.1.isEmpty then []
  else match p.2 with
    | [] => f ++ [10]
    | 10 :: _ => p.1 ++ [10]
    | 13 :: 10 :: _ => p.1 ++ [13, 10]
    | _ => p.1 ++ [13]

/-! ## snp's Rust kernel (`end_of_file_run(head, run, len)`) -/

inductive Action where
  | keep
  | append
  | truncate (n : Nat)
  deriving Repr, DecidableEq

/-- `end_of_file_run`: `head` = the first `min(run, 2)` bytes of the run. -/
def kernelRun (head : List UInt8) (run len : Nat) : Action :=
  if len = 0 then .keep
  else if run = 0 then .append
  else if run ≥ len then .truncate 0
  else
    let position := len - run
    if [10].isPrefixOf head then (if run = 1 then .keep else .truncate (position + 1))
    else if [13, 10].isPrefixOf head then (if run = 2 then .keep else .truncate (position + 2))
    else if [13].isPrefixOf head then (if run = 1 then .keep else .truncate (position + 1))
    else .keep

/-- `end_of_file(tail, len)` with `tail` = the whole file. -/
def kernel (f : List UInt8) : Action :=
  let run := (f.reverse.takeWhile isEol).length
  kernelRun ((f.drop (f.length - run)).take 2) run f.length

def Action.apply (f : List UInt8) : Action → List UInt8
  | .keep => f
  | .append => f ++ [10]
  | .truncate n => f.take n

/-! ## Lemmas about `splitTail` -/

theorem splitTail_append (f : List UInt8) : (splitTail f).1 ++ (splitTail f).2 = f := by
  induction f with
  | nil => rfl
  | cons b bs ih =>
    simp only [splitTail]
    split <;> simp_all

theorem splitTail_run_eol (f : List UInt8) : ∀ x ∈ (splitTail f).2, isEol x = true := by
  induction f with
  | nil => simp [splitTail]
  | cons b bs ih =>
    simp only [splitTail]
    split
    · next h =>
      simp only [Bool.and_eq_true, List.isEmpty_iff] at h
      intro x hx
      simp only [List.mem_cons] at hx
      rcases hx with rfl | hx
      · exact h.2
      · exact ih x hx
    · exact ih

/-- The body is empty or ends in a non-EOL byte. -/
theorem splitTail_body_last (f : List UInt8) :
    ∀ h : (splitTail f).1 ≠ [], isEol ((splitTail f).1.getLast h) = false := by
  induction f with
  | nil => simp [splitTail]
  | cons b bs ih =>
    simp only [splitTail]
    split
    · simp
    · next h =>
      intro hne
      by_cases hb : (splitTail bs).1 = []
      · simp only [hb, List.isEmpty_nil, Bool.true_and, Bool.not_eq_true] at h
        simp [hb, h]
      · rw [List.getLast_cons hb]; exact ih hb

/-- `splitTail` is characterised by the two invariants. -/
theorem splitTail_of (c r : List UInt8) (hr : ∀ x ∈ r, isEol x = true)
    (hc : ∀ h : c ≠ [], isEol (c.getLast h) = false) : splitTail (c ++ r) = (c, r) := by
  induction c with
  | nil =>
    induction r with
    | nil => rfl
    | cons x xs ih =>
      have hx : isEol x = true := hr x (by simp)
      have ih' := ih (fun y hy => hr y (by simp [hy]))
      simp only [List.nil_append] at ih' ⊢
      simp [splitTail, ih', hx]
  | cons b bs ih =>
    by_cases hbs : bs = []
    · subst hbs
      have hb : isEol b = false := by simpa using hc (by simp)
      have hr' : splitTail r = ([], r) := by
        have := ih (by simp); simpa using this
      simp [splitTail, hr', hb]
    · have hc' : ∀ h : bs ≠ [], isEol (bs.getLast h) = false := by
        intro h
        have := hc (by simp)
        rwa [List.getLast_cons h] at this
      have := ih hc'
      simp only [List.cons_append, splitTail, this]
      simp [hbs]

/-! ## The spec is idempotent and preserves the body -/

private theorem eol10 : isEol 10 = true := rfl
private theorem eol13 : isEol 13 = true := rfl

theorem fix_cases (f : List UInt8) :
    fix f = f ∨ fix f = [] ∨
    ((splitTail f).1 ≠ [] ∧ ∃ t, (t = [10] ∨ t = [13, 10] ∨ t = [13]) ∧ fix f = (splitTail f).1 ++ t) := by
  unfold fix
  by_cases he : f.isEmpty
  · simp [he]
  · simp only [he, Bool.false_eq_true, ite_false]
    by_cases hb : (splitTail f).1.isEmpty
    · simp [hb]
    · simp only [hb, Bool.false_eq_true, ite_false]
      have hb' : (splitTail f).1 ≠ [] := by simpa using hb
      split
      · next h =>
        right; right; refine ⟨hb', [10], by simp, ?_⟩
        have := splitTail_append f; rw [h] at this; simp at this
        conv => lhs; rw [← this]
      · right; right; exact ⟨hb', [10], by simp, rfl⟩
      · right; right; exact ⟨hb', [13, 10], by simp, rfl⟩
      · right; right; exact ⟨hb', [13], by simp, rfl⟩

theorem fix_idem (f : List UInt8) : fix (fix f) = fix f := by
  rcases fix_cases f with h | h | ⟨hb, t, ht, h⟩
  · rw [h, h]
  · rw [h]; rfl
  · rw [h]
    have hr : ∀ x ∈ t, isEol x = true := by
      rcases ht with rfl | rfl | rfl <;> simp [eol10, eol13]
    have hs := splitTail_of (splitTail f).1 t hr (splitTail_body_last f)
    unfold fix
    rw [hs]
    have hne : ((splitTail f).1 ++ t).isEmpty = false := by simp [hb]
    have hne2 : (splitTail f).1.isEmpty = false := by simpa using hb
    simp only [hne, hne2, Bool.false_eq_true, ite_false]
    rcases ht with rfl | rfl | rfl <;> rfl

/-- The fix only ever rewrites the trailing CR/LF run. -/
theorem fix_body (f : List UInt8) : (splitTail (fix f)).1 = (splitTail f).1 := by
  rcases fix_cases f with h | h | ⟨hb, t, ht, h⟩
  · rw [h]
  · rw [h]
    unfold fix at h
    by_cases he : f.isEmpty
    · simp at he; subst he; rfl
    · simp only [he, Bool.false_eq_true, ite_false] at h
      by_cases hb : (splitTail f).1.isEmpty
      · simp at hb; rw [hb]; rfl
      · simp only [hb, Bool.false_eq_true, ite_false] at h
        split at h <;> simp at h
  · rw [h]
    have hr : ∀ x ∈ t, isEol x = true := by
      rcases ht with rfl | rfl | rfl <;> simp [eol10, eol13]
    rw [splitTail_of _ t hr (splitTail_body_last f)]

/-! ## The Python reference equals the spec -/

private theorem getD_append_left (c r : List UInt8) (i : Nat) (h : i < c.length) :
    (c ++ r).getD i 0 = c.getD i 0 := by
  simp [List.getD_eq_getElem?_getD, List.getElem?_append_left h]

private theorem getD_append_right (c r : List UInt8) (i : Nat) (h : c.length ≤ i) :
    (c ++ r).getD i 0 = r.getD (i - c.length) 0 := by
  simp [List.getD_eq_getElem?_getD, List.getElem?_append_right h]

private theorem getD_mem (r : List UInt8) (i : Nat) (h : i < r.length) : r.getD i 0 ∈ r := by
  simp [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem h]

private theorem getD_last (c : List UInt8) (h : c ≠ []) : c.getD (c.length - 1) 0 = c.getLast h := by
  have hl : c.length - 1 < c.length := by have := List.length_pos_iff.mpr h; omega
  simp [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem hl, List.getLast_eq_getElem]

private theorem eol_cases (x : UInt8) (h : isEol x = true) : x = 10 ∨ x = 13 := by
  simpa [isEol] using h

/-- Loop invariant: every offset `≥ |body|` holds an EOL byte, so the loop walks back to the body. -/
theorem pyLoop_eq (f : List UInt8) (i : Nat) (hi : i < f.length) (hm : (splitTail f).1.length ≤ i) :
    pyFixFile.loop f i =
      if (splitTail f).1.length = 0 then ([], 1) else pyFixFile.trailer f (splitTail f).1.length := by
  induction i with
  | zero =>
    have : (splitTail f).1.length = 0 := by omega
    simp [pyFixFile.loop, this]
  | succ j ih =>
    simp only [pyFixFile.loop]
    have happ := splitTail_append f
    by_cases hj : (splitTail f).1.length ≤ j
    · have hjr : j - (splitTail f).1.length < (splitTail f).2.length := by
        have := congrArg List.length happ; simp at this; omega
      have hb : isEol (f.getD j 0) = true := by
        conv => lhs; rw [← happ]
        rw [getD_append_right _ _ _ hj]
        exact splitTail_run_eol f _ (getD_mem _ _ hjr)
      simp only [hb, ite_true]
      exact ih (by omega) hj
    · have hjeq : (splitTail f).1.length = j + 1 := by omega
      have hne : (splitTail f).1 ≠ [] := by intro h; simp [h] at hjeq
      have hb : isEol (f.getD j 0) = false := by
        have h1 : f.getD j 0 = (splitTail f).1.getD j 0 := by
          conv => lhs; rw [← happ]
          exact getD_append_left _ _ _ (by omega)
        have h2 := getD_last _ hne
        rw [hjeq, Nat.add_sub_cancel] at h2
        rw [h1, h2]; exact splitTail_body_last f hne
      simp only [hb, Bool.false_eq_true, ite_false]
      simp [hjeq]

/-- **The Python reference equals the spec**, including the return code (1 exactly when the file changes). -/
theorem pyFixFile_eq (f : List UInt8) : pyFixFile f = (fix f, if fix f = f then 0 else 1) := by
  have happ := splitTail_append f
  have hrun := splitTail_run_eol f
  have hlast := splitTail_body_last f
  have hloop := pyLoop_eq f
  generalize hc : (splitTail f).1 = c at happ hlast hloop
  generalize hr : (splitTail f).2 = r at happ hrun
  unfold pyFixFile fix
  by_cases hf : f = []
  · subst hf; simp
  have hflen : f.length ≠ 0 := by simpa using hf
  have hfe : f.isEmpty = false := by simpa using hf
  simp only [hflen, ite_false, hc, hr, hfe, Bool.false_eq_true]
  by_cases hr0 : r = []
  · -- no trailing EOL: append `\n`
    subst hr0
    simp at happ; subst happ
    rw [getD_last _ hf, hlast hf]
    simp [hf]
  · -- the last byte is EOL: run the loop
    have hlastEol : isEol (f.getD (f.length - 1) 0) = true := by
      rw [getD_last _ hf]
      have : f.getLast hf ∈ r := by
        subst happ; rw [List.getLast_append_of_ne_nil _ hr0]; exact List.getLast_mem hr0
      exact hrun _ this
    simp only [hlastEol, Bool.not_true, Bool.false_eq_true, ite_false]
    have hlen : c.length ≤ f.length - 1 := by
      have := congrArg List.length happ; simp at this
      have : r.length ≠ 0 := by simpa using hr0
      omega
    rw [hloop (f.length - 1) (by omega) hlen]
    by_cases hc0 : c = []
    · subst hc0; simp [hf]
    · have hcl : c.length ≠ 0 := by simpa using hc0
      have hce : c.isEmpty = false := by simpa using hc0
      simp only [hcl, ite_false, hce, Bool.false_eq_true]
      unfold pyFixFile.trailer
      have hdrop : f.drop c.length = r := by subst happ; simp
      have htake : ∀ k, f.take (c.length + k) = c ++ r.take k := by
        intro k; subst happ; rw [List.take_append, List.take_of_length_le (by omega)]; simp
      simp only [hdrop, htake]
      obtain ⟨x, rs, rfl⟩ := List.exists_cons_of_ne_nil hr0
      rcases eol_cases x (hrun x (by simp)) with rfl | rfl
      · cases rs with
        | nil => subst happ; simp
        | cons y ys => subst happ; simp
      · cases rs with
        | nil => subst happ; simp
        | cons y ys =>
          rcases eol_cases y (hrun y (by simp)) with rfl | rfl
          · cases ys with
            | nil => subst happ; simp
            | cons z zs => subst happ; simp
          · subst happ; simp

theorem pyFixFile_eq_fix (f : List UInt8) : (pyFixFile f).1 = fix f := by
  rw [pyFixFile_eq]

/-- Corollary: running the Python twice changes nothing the second time (returns 0). -/
theorem pyFixFile_twice (f : List UInt8) : (pyFixFile (pyFixFile f).1).2 = 0 := by
  rw [pyFixFile_eq_fix, pyFixFile_eq, fix_idem]; simp

/-! ## snp's Rust kernel equals the spec -/

theorem takeWhile_rev (f : List UInt8) : f.reverse.takeWhile isEol = (splitTail f).2.reverse := by
  conv => lhs; rw [← splitTail_append f]
  rw [List.reverse_append, List.takeWhile_append_of_pos (fun a ha => splitTail_run_eol f a (by simpa using ha))]
  have : (splitTail f).1.reverse.takeWhile isEol = [] := by
    by_cases h : (splitTail f).1 = []
    · simp [h]
    · obtain ⟨y, ys, hy⟩ := List.exists_cons_of_ne_nil (by simpa using h : (splitTail f).1.reverse ≠ [])
      have hlast := splitTail_body_last f h
      have : y = (splitTail f).1.getLast h := by
        have := List.head_reverse (l := (splitTail f).1) (by simpa using h)
        simp only [hy, List.head_cons] at this; exact this
      rw [hy, List.takeWhile_cons_of_neg (by rw [this, hlast]; simp)]
  rw [this, List.append_nil]

theorem kernel_apply (f : List UInt8) : (kernel f).apply f = fix f := by
  have happ := splitTail_append f
  have hrun := splitTail_run_eol f
  have hlast := splitTail_body_last f
  unfold kernel fix
  dsimp only
  rw [takeWhile_rev, List.length_reverse]
  generalize hc : (splitTail f).1 = c at happ hlast
  generalize hr : (splitTail f).2 = r at happ hrun
  have hlen : f.length = c.length + r.length := by subst happ; simp
  have hdrop : f.drop (f.length - r.length) = r := by subst happ; simp
  rw [hdrop]
  unfold kernelRun
  by_cases hf : f = []
  · subst hf; simp [Action.apply]
  have hflen : f.length ≠ 0 := by simpa using hf
  have hfe : f.isEmpty = false := by simpa using hf
  simp only [hflen, ite_false, hfe, Bool.false_eq_true]
  by_cases hr0 : r = []
  · subst hr0
    have hc0 : c ≠ [] := by simp at happ; subst happ; exact hf
    have hce : c.isEmpty = false := by simpa using hc0
    simp [Action.apply, hce]
  · have hrl : r.length ≠ 0 := by simpa using hr0
    simp only [hrl, ite_false]
    by_cases hc0 : c = []
    · subst hc0; simp [Action.apply, hlen]
    · have hcl : c.length ≠ 0 := by simpa using hc0
      have hce : c.isEmpty = false := by simpa using hc0
      have hnotge : ¬ (r.length ≥ f.length) := by omega
      have hpos : f.length - r.length = c.length := by omega
      simp only [hnotge, ite_false, hce, Bool.false_eq_true, hpos]
      have htake : ∀ k, f.take (c.length + k) = c ++ r.take k := by
        intro k; subst happ; rw [List.take_append, List.take_of_length_le (by omega)]; simp
      obtain ⟨x, rs, rfl⟩ := List.exists_cons_of_ne_nil hr0
      rcases eol_cases x (hrun x (by simp)) with rfl | rfl
      · cases rs with
        | nil => subst happ; simp [Action.apply]
        | cons y ys => simp [Action.apply, htake]
      · cases rs with
        | nil => subst happ; simp [Action.apply]
        | cons y ys =>
          rcases eol_cases y (hrun y (by simp)) with rfl | rfl
          · cases ys with
            | nil => subst happ; simp [Action.apply]
            | cons z zs => simp [Action.apply, htake]
          · simp [Action.apply, htake]

/-! ## Executable entry points (used by `eof-model`) -/

/-- What the CLI prints: the fixed bytes and the Python return code. -/
def run (input : ByteArray) : ByteArray × Nat :=
  let r := pyFixFile input.toList
  (ByteArray.mk r.1.toArray, r.2)

end SnpModels.Eof
