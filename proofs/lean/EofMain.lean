import SnpModels.Eof
import SnpModels.Hex

open SnpModels

/-- Read all of stdin as bytes. -/
partial def readAll (h : IO.FS.Stream) (acc : ByteArray := .empty) : IO ByteArray := do
  let chunk ← h.read 65536
  if chunk.isEmpty then return acc else readAll h (acc ++ chunk)

/-- One `--lines` reply: `OK <code> :<hex of new bytes>` from the proved Python transliteration (`pyFixFile`,
equal to `fix` by `pyFixFile_eq`), or `MISMATCH` if snp's kernel model disagrees (`kernel_apply` says it never
does; the check guards the executable, not the theorem). -/
def answer (bytes : ByteArray) : String :=
  let (out, code) := Eof.run bytes
  let f := bytes.toList
  if (Eof.kernel f).apply f != out.toList then "MISMATCH"
  else s!"OK {code} :{Hex.encode out}"

/-- `--lines`: one hex-encoded file per stdin line → one `answer` per stdout line (flushed), so a Rust test can
keep one child process alive for a whole run (proofs/lean/client.rs). -/
partial def lineLoop (stdin : IO.FS.Stream) (stdout : IO.FS.Stream) : IO Unit := do
  let line ← stdin.getLine
  if line.isEmpty then return
  match Hex.decode (line.trimAsciiEnd.toString) with
  | none => stdout.putStrLn "ERR bad-hex"
  | some bytes => stdout.putStrLn (answer bytes)
  stdout.flush
  lineLoop stdin stdout

/-- Default: stdin bytes → fixed bytes on stdout; the exit status is the Python return value (1 = changed). -/
def main (args : List String) : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  if args == ["--lines"] then
    lineLoop stdin stdout
    return 0
  let input ← readAll stdin
  let (out, code) := Eof.run input
  stdout.write out
  stdout.flush
  return code.toUInt32
