/-! Hex framing for the line protocol shared by the model executables (one record per line). -/
namespace SnpModels.Hex

def hexDigit (n : Nat) : Char := "0123456789abcdef".toList.getD n '0'

def encode (b : ByteArray) : String := Id.run do
  let mut s := ""
  for x in b.data do
    s := (s.push (hexDigit (x.toNat / 16))).push (hexDigit (x.toNat % 16))
  return s

def nibble (c : Char) : Option Nat :=
  if '0' ≤ c ∧ c ≤ '9' then some (c.toNat - '0'.toNat)
  else if 'a' ≤ c ∧ c ≤ 'f' then some (c.toNat - 'a'.toNat + 10)
  else if 'A' ≤ c ∧ c ≤ 'F' then some (c.toNat - 'A'.toNat + 10)
  else none

def decode (s : String) : Option ByteArray :=
  let rec go : List Char → ByteArray → Option ByteArray
    | [], acc => some acc
    | a :: b :: rest, acc => do
      let hi ← nibble a
      let lo ← nibble b
      go rest (acc.push (UInt8.ofNat (hi * 16 + lo)))
    | [_], _ => none
  go s.toList ByteArray.empty

end SnpModels.Hex
