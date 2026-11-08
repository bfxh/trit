import Trit

open TritFormal

def k3Name : K3 → String
  | .F => "F"
  | .U => "U"
  | .T => "T"

def k3Id : K3 → String
  | .F => "f"
  | .U => "u"
  | .T => "t"

def b4Name : B4 → String
  | .N => "N"
  | .T => "T"
  | .F => "F"
  | .B => "B"

def b4Id : B4 → String
  | .N => "n"
  | .T => "t"
  | .F => "f"
  | .B => "b"

def tritName : PackTrit → String
  | .neg => "-1"
  | .zero => "0"
  | .pos => "1"

def bitName (b : Bool) : String :=
  if b then "1" else "0"

def pad3 (n : Nat) : String :=
  let s := toString n
  if s.length = 1 then "00" ++ s
  else if s.length = 2 then "0" ++ s
  else s

def joinComma : List String → String
  | [] => ""
  | x :: [] => x
  | x :: xs => x ++ "," ++ joinComma xs

def emitStr (id val : String) : IO Unit :=
  IO.println ("{\"id\":\"" ++ id ++ "\",\"expected\":\"" ++ val ++ "\"}")

def emitNat (id : String) (n : Nat) : IO Unit :=
  IO.println ("{\"id\":\"" ++ id ++ "\",\"expected\":" ++ toString n ++ "}")

def emitBits (id : String) (b : Bool × Bool) : IO Unit :=
  IO.println ("{\"id\":\"" ++ id ++ "\",\"expected\":[" ++ bitName b.1 ++ "," ++ bitName b.2 ++ "]}")

def emitTrits (id : String) (ts : List PackTrit) : IO Unit :=
  IO.println ("{\"id\":\"" ++ id ++ "\",\"expected\":[" ++ joinComma (ts.map tritName) ++ "]}")

def emitError (id err : String) : IO Unit :=
  IO.println ("{\"id\":\"" ++ id ++ "\",\"error\":\"" ++ err ++ "\"}")

def k3Values : List K3 := [.F, .U, .T]
def b4Values : List B4 := [.N, .T, .F, .B]
def allTrits : List PackTrit := [.neg, .zero, .pos]

def concatMap {α β : Type} (f : α → List β) : List α → List β
  | [] => []
  | x :: xs => f x ++ concatMap f xs

def listsOfLen : Nat → List (List PackTrit)
  | 0 => [[]]
  | n + 1 => concatMap (fun xs => allTrits.map fun t => xs ++ [t]) (listsOfLen n)

def main : IO Unit := do
  -- K3 semantic
  for a in k3Values do
    emitStr s!"k3.not.{k3Id a}" (k3Name (K3.not a))
  for a in k3Values do
    for b in k3Values do
      emitStr s!"k3.and.{k3Id a}.{k3Id b}" (k3Name (K3.and a b))
      emitStr s!"k3.or.{k3Id a}.{k3Id b}" (k3Name (K3.or a b))

  -- K3 bitplane
  for a in k3Values do
    emitBits s!"k3.bitplane.encode.{k3Id a}" (K3.toBits a)
  emitStr "k3.bitplane.decode.00" "F"
  emitStr "k3.bitplane.decode.10" "T"
  emitStr "k3.bitplane.decode.01" "U"
  emitError "k3.bitplane.decode.invalid.11" "invalid_bitplane"

  -- KGate
  for r in k3Values do
    for v in k3Values do
      emitStr s!"kgate.{k3Id r}.{k3Id v}" (k3Name (K3.gate r v))

  -- Belnap4 semantic
  for a in b4Values do
    emitStr s!"belnap4.not.{b4Id a}" (b4Name (B4.not a))
  for a in b4Values do
    for b in b4Values do
      emitStr s!"belnap4.and.{b4Id a}.{b4Id b}" (b4Name (B4.and a b))
      emitStr s!"belnap4.or.{b4Id a}.{b4Id b}" (b4Name (B4.or a b))

  -- Belnap4 bitplane
  for a in b4Values do
    emitBits s!"belnap4.bitplane.encode.{b4Id a}" (B4.toBits a)
  emitStr "belnap4.bitplane.decode.00" "N"
  emitStr "belnap4.bitplane.decode.10" "T"
  emitStr "belnap4.bitplane.decode.01" "F"
  emitStr "belnap4.bitplane.decode.11" "B"

  -- pack5: full groups and tails
  for ts in listsOfLen 5 do
    let b := Pack5.packGroup ts
    emitNat s!"pack5.pack.{pad3 b}" b
  for len in [1, 2, 3, 4] do
    for ts in listsOfLen len do
      let b := Pack5.packGroup ts
      emitNat s!"pack5.pack.tail.{len}.{pad3 b}" b

  -- pack5 unpack: all valid bytes
  for b in List.range 243 do
    match Pack5.unpackGroup b 5 with
    | .ok ts => emitTrits s!"pack5.unpack.{pad3 b}" ts
    | .error _ => emitError s!"pack5.unpack.{pad3 b}" "unexpected"

  -- pack5 unpack tails: the representative bytes used by the vectors
  let tailBytes : List Nat := [0, 1, 2, 3, 40, 80, 121, 122, 242]
  for len in [1, 2, 3, 4] do
    for b in tailBytes do
      match Pack5.unpackGroup b len with
      | .ok ts => emitTrits s!"pack5.unpack.tail.{len}.{pad3 b}" ts
      | .error _ => emitError s!"pack5.unpack.tail.{len}.{pad3 b}" "unexpected"

  -- pack5 errors
  for i in List.range 13 do
    let b := 243 + i
    emitError s!"pack5.unpack.invalid.{pad3 b}" "invalid_byte"
  emitError "pack5.unpack.short_input.006.001" "short_input"
  match Pack5.ofInt 2 with
  | .error .invalidTrit => emitError "pack5.pack.invalid_trit.2" "invalid_trit"
  | _ => emitError "pack5.pack.invalid_trit.2" "unexpected"
