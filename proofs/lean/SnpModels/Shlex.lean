/-!
# `shlex.split` / `shlex.quote` reference model

`shlex.split(s)` = `shlex(s, posix=True)` with `whitespace_split = True`, `commenters = ''` (CPython `Lib/shlex.py`).

* `pySplit` – a transliteration of `read_token` + `__next__` (the *reference*), Python's state characters kept;
* `split`   – the one-pass state machine snp implements (`snp_pycompat::shlex::split`);
* `quote`   – `shlex.quote`.

Proved: `split (quote s) = .ok [s]` for every string.
-/

namespace SnpModels.Shlex

inductive Err where
  | noClosingQuotation
  | noEscapedCharacter
  deriving Repr, DecidableEq

def Err.message : Err → String
  | .noClosingQuotation => "No closing quotation"
  | .noEscapedCharacter => "No escaped character"

/-! ## Reference: `read_token` (posix, whitespace_split, no commenters, no punctuation_chars) -/

def whitespace : List Char := [' ', '\t', '\r', '\n']
def quotes : List Char := ['\'', '"']
def escape : List Char := ['\\']
def escapedquotes : List Char := ['"']

/-- The lexer object: `self.state` (`none` = Python `None`), `self.token`, the unread input. -/
structure PyLex where
  state : Option Char
  token : List Char
  instream : List Char

/-- The `while True` loop of `read_token`; `nextchar = instream.read(1)` is the head of the list (`''` = `[]`).
Returns `(self.state, self.token, quoted, rest)` at `break`. Every iteration that does not break consumes a
character, so this is structural recursion. -/
def readLoop (state : Option Char) (token : List Char) (quoted : Bool) (escapedstate : Char) :
    List Char → Except Err (Option Char × List Char × Bool × List Char)
  | [] =>
    match state with
    | none => .ok (none, [], quoted, [])                         -- self.token = ''; break
    | some st =>
      if st == ' ' then .ok (none, token, quoted, [])            -- self.state = None; break
      else if st ∈ quotes then .error .noClosingQuotation        -- raise ValueError("No closing quotation")
      else if st ∈ escape then .error .noEscapedCharacter        -- raise ValueError("No escaped character")
      else .ok (none, token, quoted, [])                         -- 'a': self.state = None; break
  | c :: rest =>
    match state with
    | none => .ok (none, [], quoted, rest)
    | some st =>
      if st == ' ' then
        if c ∈ whitespace then
          if !token.isEmpty || quoted then .ok (some ' ', token, quoted, rest)    -- break
          else readLoop (some ' ') token quoted escapedstate rest                -- continue
        else if c ∈ escape then readLoop (some c) token quoted 'a' rest           -- escapedstate = 'a'
        else if c ∈ quotes then readLoop (some c) token quoted escapedstate rest  -- posix: token unchanged
        else readLoop (some 'a') [c] quoted escapedstate rest                     -- wordchars / whitespace_split
      else if st ∈ quotes then                                                    -- quoted = True
        if c == st then readLoop (some 'a') token true escapedstate rest
        else if c ∈ escape && st ∈ escapedquotes then readLoop (some c) token true st rest
        else readLoop (some st) (token ++ [c]) true escapedstate rest
      else if st ∈ escape then
        let token := if escapedstate ∈ quotes && c != st && c != escapedstate then token ++ [st] else token
        readLoop (some escapedstate) (token ++ [c]) quoted escapedstate rest
      else                                                                        -- state 'a'
        if c ∈ whitespace then
          if !token.isEmpty || quoted then .ok (some ' ', token, quoted, rest)
          else readLoop (some ' ') token quoted escapedstate rest
        else if c ∈ quotes then readLoop (some c) token quoted escapedstate rest
        else if c ∈ escape then readLoop (some c) token quoted 'a' rest
        else readLoop (some 'a') (token ++ [c]) quoted escapedstate rest

/-- `read_token()`: `none` is `self.eof` (posix: an unquoted empty result). -/
def readToken (lx : PyLex) : Except Err (Option (List Char) × PyLex) := do
  let (state, result, quoted, rest) ← readLoop lx.state lx.token false ' ' lx.instream
  let tok := if !quoted && result.isEmpty then none else some result
  return (tok, { state, token := [], instream := rest })

/-- `list(lex)`; `fuel` bounds the iterations (each token consumes at least one character). -/
def pySplitAux : Nat → PyLex → Except Err (List (List Char))
  | 0, _ => .ok []
  | n + 1, lx => do
    match ← readToken lx with
    | (none, _) => return []
    | (some t, lx') => return t :: (← pySplitAux n lx')

def pySplit (s : List Char) : Except Err (List (List Char)) :=
  pySplitAux (s.length + 1) { state := some ' ', token := [], instream := s }

/-! ## The one-pass state machine (snp's Rust `split`) -/

inductive St where
  | ws
  | word
  | quote (q : Char)
  | escWord
  | escDouble
  deriving Repr, DecidableEq

structure Fsm where
  st : St
  tok : List Char
  quoted : Bool
  out : List (List Char)

def isWs (c : Char) : Bool := c == ' ' || c == '\t' || c == '\r' || c == '\n'

def flush (m : Fsm) : List (List Char) := if !m.tok.isEmpty || m.quoted then m.out ++ [m.tok] else m.out

def step (m : Fsm) (c : Char) : Fsm :=
  match m.st with
  | .ws =>
    if isWs c then m
    else if c == '\\' then { m with st := .escWord }
    else if c == '\'' || c == '"' then { m with st := .quote c }
    else { m with tok := m.tok ++ [c], st := .word }
  | .word =>
    if isWs c then { st := .ws, tok := [], quoted := false, out := flush m }
    else if c == '\'' || c == '"' then { m with st := .quote c }
    else if c == '\\' then { m with st := .escWord }
    else { m with tok := m.tok ++ [c] }
  | .quote q =>
    if c == q then { m with st := .word, quoted := true }
    else if q == '"' && c == '\\' then { m with st := .escDouble, quoted := true }
    else { m with tok := m.tok ++ [c], quoted := true }
  | .escWord => { m with tok := m.tok ++ [c], st := .word }
  | .escDouble =>
    { m with tok := (if c != '\\' && c != '"' then m.tok ++ ['\\'] else m.tok) ++ [c], st := .quote '"' }

def init : Fsm := { st := .ws, tok := [], quoted := false, out := [] }

def finish (m : Fsm) : Except Err (List (List Char)) :=
  match m.st with
  | .quote _ => .error .noClosingQuotation
  | .escWord | .escDouble => .error .noEscapedCharacter
  | _ => .ok (flush m)

def split (s : List Char) : Except Err (List (List Char)) := finish (s.foldl step init)

/-! ## `shlex.quote` -/

/-- `_find_unsafe` complement: ASCII letters, digits and `_@%+=:,.-` plus slash. -/
def isSafe (c : Char) : Bool := c.isAlphanum || c ∈ ['_', '@', '%', '+', '=', ':', ',', '.', '/', '-']

def escQ (c : Char) : List Char := if c = '\'' then ['\'', '"', '\'', '"', '\''] else [c]

/-- `"'" + s.replace("'", "'\"'\"'") + "'"` unless `s` is empty or all-safe. -/
def quote (s : List Char) : List Char :=
  if s.isEmpty then ['\'', '\'']
  else if s.all isSafe then s
  else '\'' :: s.flatMap escQ ++ ['\'']

def join (args : List (List Char)) : List Char := (args.map quote).intersperse [' '] |>.flatten

/-! ## Proof: `split (quote s) = .ok [s]` -/

/-- The five characters `'"'"'` that replace a `'` leave the quote, append `'` and re-enter `'…'`. -/
theorem escQ_quote (t : List Char) (b : Bool) (o : List (List Char)) :
    (escQ '\'').foldl step ⟨.quote '\'', t, b, o⟩ = ⟨.quote '\'', t ++ ['\''], true, o⟩ := by
  simp [escQ, step, isWs]

/-- Inside `'…'`, the escaped body of `s` appends `s` to the token and stays in the quote. -/
theorem quoted_body (s t : List Char) (b : Bool) (o : List (List Char)) :
    ∃ b', (s.flatMap escQ).foldl step ⟨.quote '\'', t, b, o⟩ = ⟨.quote '\'', t ++ s, b', o⟩ := by
  induction s generalizing t b with
  | nil => exact ⟨b, by simp⟩
  | cons c cs ih =>
    rw [List.flatMap_cons, List.foldl_append]
    by_cases hc : c = '\''
    · subst hc
      obtain ⟨b', h⟩ := ih (t ++ ['\'']) true
      refine ⟨b', ?_⟩
      rw [escQ_quote, h]; simp
    · obtain ⟨b', h⟩ := ih (t ++ [c]) true
      refine ⟨b', ?_⟩
      have hc' : (c == '\'') = false := by simpa using hc
      simp only [escQ, hc, ite_false, List.foldl_cons, List.foldl_nil, step, hc', Bool.false_eq_true]
      simpa using h

/-- Safe characters are not separators, quotes or backslashes. -/
theorem safe_not_special (c : Char) (h : isSafe c = true) :
    isWs c = false ∧ (c == '\'' || c == '"') = false ∧ (c == '\\') = false := by
  have h' : ∀ x, isSafe x = false → c ≠ x := by intro x hx hcx; subst hcx; simp_all
  have n1 := h' ' ' (by decide)
  have n2 := h' '\t' (by decide)
  have n3 := h' '\r' (by decide)
  have n4 := h' '\n' (by decide)
  have n5 := h' '\'' (by decide)
  have n6 := h' '"' (by decide)
  have n7 := h' '\\' (by decide)
  simp [isWs, n1, n2, n3, n4, n5, n6, n7]

/-- Safe characters in the word state are appended verbatim. -/
theorem safe_word (s t : List Char) (hs : s.all isSafe = true) (b : Bool) (o : List (List Char)) :
    s.foldl step ⟨.word, t, b, o⟩ = ⟨.word, t ++ s, b, o⟩ := by
  induction s generalizing t with
  | nil => simp
  | cons c cs ih =>
    simp only [List.all_cons, Bool.and_eq_true] at hs
    obtain ⟨h1, h2, h3⟩ := safe_not_special c hs.1
    have : step ⟨.word, t, b, o⟩ c = ⟨.word, t ++ [c], b, o⟩ := by
      simp [step, h1, h2, h3]
    rw [List.foldl_cons, this, ih _ hs.2, List.append_assoc, List.singleton_append]

/-- **Round trip**: `shlex.split(shlex.quote(s)) == [s]` for every string `s`. -/
theorem split_quote (s : List Char) : split (quote s) = .ok [s] := by
  unfold quote
  by_cases he : s.isEmpty
  · simp at he; subst he; rfl
  · simp only [he, Bool.false_eq_true, ite_false]
    by_cases hs : s.all isSafe
    · simp only [hs, ite_true]
      obtain ⟨c, cs, rfl⟩ := List.exists_cons_of_ne_nil (by simpa using he : s ≠ [])
      simp only [List.all_cons, Bool.and_eq_true] at hs
      obtain ⟨h1, h2, h3⟩ := safe_not_special c hs.1
      have h0 : step init c = ⟨.word, [c], false, []⟩ := by simp [init, step, h1, h2, h3]
      unfold split
      rw [List.foldl_cons, h0, safe_word cs [c] hs.2]
      simp [finish, flush]
    · simp only [hs, Bool.false_eq_true, ite_false]
      obtain ⟨b', h⟩ := quoted_body s [] false []
      unfold split
      simp only [List.foldl_append, List.foldl_cons, List.foldl_nil]
      have h0 : step init '\'' = ⟨.quote '\'', [], false, []⟩ := rfl
      rw [h0, h]
      simp [step, finish, flush]

/-! ## Proof: the Python transliteration equals the one-pass state machine -/

/-- Python `(self.state, escapedstate)` ↔ the state-machine state (`escapedstate` matters only in the escape). -/
inductive Corr : Option Char → Char → St → Prop
  | ws (e : Char) : Corr (some ' ') e .ws
  | word (e : Char) : Corr (some 'a') e .word
  | sq (e : Char) : Corr (some '\'') e (.quote '\'')
  | dq (e : Char) : Corr (some '"') e (.quote '"')
  | escW : Corr (some '\\') 'a' .escWord
  | escD : Corr (some '\\') '"' .escDouble

/-- What `pySplitAux (n + 1)` does with the result of one `readLoop`. -/
def cont (n : Nat) : Option Char × List Char × Bool × List Char → Except Err (List (List Char))
  | (st', res, q, rest) =>
    if !q && res.isEmpty then pure [] else do return res :: (← pySplitAux n ⟨st', [], rest⟩)

theorem pySplitAux_succ (n : Nat) (lx : PyLex) :
    pySplitAux (n + 1) lx = readLoop lx.state lx.token false ' ' lx.instream >>= cont n := by
  simp only [pySplitAux, readToken]
  cases readLoop lx.state lx.token false ' ' lx.instream with
  | error e => rfl
  | ok r =>
    obtain ⟨st', res, q, rest⟩ := r
    simp only [bind, Except.bind, pure, Except.pure, cont]
    by_cases h : (!q && res.isEmpty) = true
    · simp [h]
    · simp [h]

theorem ok_bind {α β : Type} (x : α) (f : α → Except Err β) : (Except.ok x >>= f) = f x := rfl

theorem pySplitAux_eof (n : Nat) : pySplitAux n ⟨none, [], []⟩ = .ok [] := by
  cases n <;> rfl

theorem map_bind_cons (f : List (List Char) → List (List Char)) (t : List Char)
    (x : Except Err (List (List Char))) :
    Except.map f (do return t :: (← x)) = Except.map (fun r => f (t :: r)) x := by
  cases x <;> rfl

private theorem char_cases (c : Char) :
    (c ∈ whitespace ∧ c ≠ '\\' ∧ c ≠ '\'' ∧ c ≠ '"') ∨ c = '\\' ∨ c = '\'' ∨ c = '"' ∨
    (c ∉ whitespace ∧ c ≠ '\\' ∧ c ≠ '\'' ∧ c ≠ '"') := by
  by_cases h1 : c = '\\'
  · exact .inr (.inl h1)
  by_cases h2 : c = '\''
  · exact .inr (.inr (.inl h2))
  by_cases h3 : c = '"'
  · exact .inr (.inr (.inr (.inl h3)))
  by_cases h : c ∈ whitespace
  · exact .inl ⟨h, h1, h2, h3⟩
  · exact .inr (.inr (.inr (.inr ⟨h, h1, h2, h3⟩)))

private theorem isWs_iff (c : Char) : isWs c = true ↔ c ∈ whitespace := by
  simp [isWs, whitespace, or_assoc]

/-- One token of `read_token` (and everything after it) is the state machine run on the same characters. -/
theorem readLoop_fsm (s : List Char) : ∀ (state : Option Char) (esc : Char) (m : Fsm) (n : Nat),
    Corr state esc m.st → (m.st = .ws → m.tok = [] ∧ m.quoted = false) → s.length ≤ n →
    Except.map (m.out ++ ·) (readLoop state m.tok m.quoted esc s >>= cont n) = finish (s.foldl step m) := by
  induction s with
  | nil =>
    intro state e m n hc hws _
    obtain ⟨st, tok, q, out⟩ := m
    simp only at hc hws
    cases hc with
    | ws _ =>
      obtain ⟨rfl, rfl⟩ := hws rfl
      simp [readLoop, cont, finish, flush, bind, Except.bind, pure, Except.pure, Except.map]
    | word _ =>
      have hr : readLoop (some 'a') tok q e [] = .ok (none, tok, q, []) := by simp [readLoop, quotes, escape]
      simp only [List.foldl_nil, finish, flush]
      rw [hr, ok_bind]
      cases q <;> cases tok <;> simp [cont, pySplitAux_eof, Except.map, pure, Except.pure, bind, Except.bind]
    | sq _ => simp [readLoop, quotes, bind, Except.bind, finish, Except.map]
    | dq _ => simp [readLoop, quotes, bind, Except.bind, finish, Except.map]
    | escW => simp [readLoop, quotes, escape, bind, Except.bind, finish, Except.map]
    | escD => simp [readLoop, quotes, escape, bind, Except.bind, finish, Except.map]
  | cons c cs ih =>
    intro state e m n hc hws hn
    obtain ⟨st, tok, q, out⟩ := m
    simp only at hc hws
    simp only [List.length_cons] at hn
    have hn' : cs.length ≤ n := by omega
    rw [List.foldl_cons]
    cases hc with
    | ws _ =>
      obtain ⟨rfl, rfl⟩ := hws rfl
      rcases char_cases c with ⟨h, h1, h2, h3⟩ | rfl | rfl | rfl | ⟨h, h1, h2, h3⟩
      · have hw : isWs c = true := (isWs_iff c).2 h
        simp only [readLoop, h, step, hw]
        simpa using ih (some ' ') e ⟨.ws, [], false, out⟩ n (.ws e) (fun _ => ⟨rfl, rfl⟩) hn'
      · simpa [readLoop, step, whitespace, escape, isWs] using ih _ _ ⟨.escWord, [], false, out⟩ n .escW (by simp) hn'
      · simpa [readLoop, step, whitespace, escape, quotes, isWs] using
          ih _ e ⟨.quote '\'', [], false, out⟩ n (.sq e) (by simp) hn'
      · simpa [readLoop, step, whitespace, escape, quotes, isWs] using
          ih _ e ⟨.quote '"', [], false, out⟩ n (.dq e) (by simp) hn'
      · have hw : isWs c = false := Bool.eq_false_iff.mpr (fun hw => h ((isWs_iff c).1 hw))
        have := ih (some 'a') e ⟨.word, [c], false, out⟩ n (.word e) (by simp) hn'
        simpa [readLoop, step, h, h1, h2, h3, hw, escape, quotes] using this
    | sq _ =>
      rcases char_cases c with ⟨h, h1, h2, h3⟩ | rfl | rfl | rfl | ⟨h, h1, h2, h3⟩
      · have := ih (some '\'') e ⟨.quote '\'', tok ++ [c], true, out⟩ n (.sq e) (by simp) hn'
        simpa [readLoop, step, h1, h2, h3, quotes, escape, escapedquotes] using this
      · have := ih (some '\'') e ⟨.quote '\'', tok ++ ['\\'], true, out⟩ n (.sq e) (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some 'a') e ⟨.word, tok, true, out⟩ n (.word e) (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some '\'') e ⟨.quote '\'', tok ++ ['"'], true, out⟩ n (.sq e) (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some '\'') e ⟨.quote '\'', tok ++ [c], true, out⟩ n (.sq e) (by simp) hn'
        simpa [readLoop, step, h1, h2, h3, quotes, escape, escapedquotes] using this
    | dq _ =>
      rcases char_cases c with ⟨h, h1, h2, h3⟩ | rfl | rfl | rfl | ⟨h, h1, h2, h3⟩
      · have := ih (some '"') e ⟨.quote '"', tok ++ [c], true, out⟩ n (.dq e) (by simp) hn'
        simpa [readLoop, step, h1, h2, h3, quotes, escape, escapedquotes] using this
      · have := ih (some '\\') '"' ⟨.escDouble, tok, true, out⟩ n .escD (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some '"') e ⟨.quote '"', tok ++ ['\''], true, out⟩ n (.dq e) (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some 'a') e ⟨.word, tok, true, out⟩ n (.word e) (by simp) hn'
        simpa [readLoop, step, quotes, escape, escapedquotes] using this
      · have := ih (some '"') e ⟨.quote '"', tok ++ [c], true, out⟩ n (.dq e) (by simp) hn'
        simpa [readLoop, step, h1, h2, h3, quotes, escape, escapedquotes] using this
    | escW =>
      have := ih (some 'a') 'a' ⟨.word, tok ++ [c], q, out⟩ n (.word 'a') (by simp) hn'
      simpa [readLoop, step, quotes, escape] using this
    | escD =>
      by_cases hc : c = '\\' ∨ c = '"'
      · have := ih (some '"') '"' ⟨.quote '"', tok ++ [c], q, out⟩ n (.dq '"') (by simp) hn'
        rcases hc with rfl | rfl <;> simpa [readLoop, step, quotes, escape] using this
      · have := ih (some '"') '"' ⟨.quote '"', tok ++ ['\\'] ++ [c], q, out⟩ n (.dq '"') (by simp) hn'
        simp only [not_or] at hc
        simpa [readLoop, step, quotes, escape, hc.1, hc.2] using this
    | word _ =>
      rcases char_cases c with ⟨h, h1, h2, h3⟩ | rfl | rfl | rfl | ⟨h, h1, h2, h3⟩
      · have hw : isWs c = true := (isWs_iff c).2 h
        by_cases hb : (!tok.isEmpty || q) = true
        · -- `break`: emit the token, then `pySplitAux` resumes in the whitespace state
          obtain ⟨k, rfl⟩ : ∃ k, n = k + 1 := ⟨n - 1, by omega⟩
          have hk : cs.length ≤ k := by omega
          have := ih (some ' ') ' ' ⟨.ws, [], false, out ++ [tok]⟩ k (.ws ' ') (fun _ => ⟨rfl, rfl⟩) hk
          have hnb : (!q && tok.isEmpty) = false := by cases q <;> simp_all
          have hr : readLoop (some 'a') tok q e (c :: cs) = .ok (some ' ', tok, q, cs) := by
            simp [readLoop, quotes, escape, h, hb]
          have hs : step ⟨.word, tok, q, out⟩ c = ⟨.ws, [], false, out ++ [tok]⟩ := by
            simp [step, hw, flush, hb]
          rw [hr, ok_bind, hs]
          simp only [cont, hnb, Bool.false_eq_true, ite_false]
          rw [map_bind_cons, pySplitAux_succ]
          simpa [List.append_assoc] using this
        · have hb' : tok = [] ∧ q = false := by cases q <;> simp_all
          obtain ⟨rfl, rfl⟩ := hb'
          have := ih (some ' ') e ⟨.ws, [], false, out⟩ n (.ws e) (fun _ => ⟨rfl, rfl⟩) hn'
          simpa [readLoop, quotes, escape, h, step, hw, flush] using this
      · have := ih (some '\\') 'a' ⟨.escWord, tok, q, out⟩ n .escW (by simp) hn'
        simpa [readLoop, step, whitespace, quotes, escape, isWs] using this
      · have := ih (some '\'') e ⟨.quote '\'', tok, q, out⟩ n (.sq e) (by simp) hn'
        simpa [readLoop, step, whitespace, quotes, escape, isWs] using this
      · have := ih (some '"') e ⟨.quote '"', tok, q, out⟩ n (.dq e) (by simp) hn'
        simpa [readLoop, step, whitespace, quotes, escape, isWs] using this
      · have hw : isWs c = false := Bool.eq_false_iff.mpr (fun hw => h ((isWs_iff c).1 hw))
        have := ih (some 'a') e ⟨.word, tok ++ [c], q, out⟩ n (.word e) (by simp) hn'
        simpa [readLoop, step, h, h1, h2, h3, hw, escape, quotes] using this

/-- **The efficient state machine is `shlex.split`**: equal results and equal errors on every input. -/
theorem pySplit_eq_split (s : List Char) : pySplit s = split s := by
  have := readLoop_fsm s (some ' ') ' ' init s.length (.ws ' ') (fun _ => ⟨rfl, rfl⟩) (Nat.le_refl _)
  have hid : ∀ x : Except Err (List (List Char)), Except.map (fun r => [] ++ r) x = x := by
    intro x; cases x <;> rfl
  unfold pySplit split
  rw [pySplitAux_succ]
  exact (hid _).symm.trans this

/-! ## Executable entry points (used by `shlex-model`) -/

def splitString (s : String) : Except Err (List String) :=
  (pySplit s.toList).map (·.map String.ofList)

def splitStringFsm (s : String) : Except Err (List String) :=
  (split s.toList).map (·.map String.ofList)

def quoteString (s : String) : String := String.ofList (quote s.toList)

end SnpModels.Shlex
