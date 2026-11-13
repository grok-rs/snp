import SnpModels.Shlex
import SnpModels.Hex

open SnpModels

partial def readAll (h : IO.FS.Stream) (acc : ByteArray := .empty) : IO ByteArray := do
  let chunk ← h.read 65536
  if chunk.isEmpty then return acc else readAll h (acc ++ chunk)

def encTokens (ts : List String) : String :=
  ts.foldl (fun acc t => acc ++ " :" ++ Hex.encode t.toUTF8) s!"OK {ts.length}"

def answer (op : String) (arg : ByteArray) : String :=
  match String.fromUTF8? arg with
  | none => "ERR bad-utf8"
  | some s =>
    if op == "split" then
      let ref := Shlex.splitString s
      -- self-check: the Python transliteration and the proved state machine must agree
      if ref.toOption != (Shlex.splitStringFsm s).toOption then "MISMATCH"
      else match ref with
        | .ok ts => encTokens ts
        | .error e => s!"ERR {e.message}"
    else if op == "quote" then encTokens [Shlex.quoteString s]
    else "ERR bad-op"

/-- `join <n> :<hex>…` (the reply framing): `shlex.join` of the `n` decoded UTF-8 arguments. -/
def answerJoin (fields : List String) : String :=
  match fields with
  | n :: args =>
    let decoded := args.mapM fun a =>
      if a.startsWith ":" then (Hex.decode (a.drop 1).toString).bind String.fromUTF8? else none
    match decoded with
    | some ts => if n == toString ts.length then encTokens [Shlex.joinStrings ts] else "ERR bad-count"
    | none => "ERR bad-hex"
  | [] => "ERR bad-count"

/-- `--lines`: `split <hex utf8>` / `quote <hex utf8>` / `join <n> :<hex>…` per line → `OK <n> :<hex>…` or
`ERR <msg>` (flushed). -/
partial def lineLoop (stdin stdout : IO.FS.Stream) : IO Unit := do
  let line ← stdin.getLine
  if line.isEmpty then return
  let line := line.trimAsciiEnd.toString
  let reply := match line.splitOn " " with
    | "join" :: fields => answerJoin fields
    | fields =>
      let (op, hex) := match fields with
        | [op, hex] => (op, hex)
        | [op] => (op, "")
        | _ => ("", "")
      match Hex.decode hex with
      | none => "ERR bad-hex"
      | some bytes => answer op bytes
  stdout.putStrLn reply
  stdout.flush
  lineLoop stdin stdout

/-- Default: `shlex.split(stdin)`, tokens NUL-terminated on stdout; `ValueError` → stderr, exit 1. -/
def main (args : List String) : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  if args == ["--lines"] then
    lineLoop stdin stdout
    return 0
  let input ← readAll stdin
  match String.fromUTF8? input with
  | none => IO.eprintln "error: stdin is not UTF-8"; return 2
  | some s =>
    match Shlex.splitString s with
    | .ok ts =>
      for t in ts do
        stdout.write t.toUTF8
        stdout.write (ByteArray.mk #[0])
      return 0
    | .error e => IO.eprintln s!"ValueError: {e.message}"; return 1
