namespace Nulang.Capabilities

inductive Cap where
  | lineariso | linear | iso | trn
  | ref | val | box | tag
  deriving BEq, Repr, Inhabited

def subtype : Cap → Cap → Bool
  | .lineariso, .iso    => true
  | .lineariso, .linear => true
  | .lineariso, .trn    => true
  | .lineariso, .ref    => true
  | .lineariso, .val    => true
  | .lineariso, .box    => true
  | .lineariso, .tag    => true
  | .linear, .val       => true
  | .linear, .box       => true
  | .linear, .tag       => true
  | .iso, .trn          => true
  | .iso, .ref          => true
  | .iso, .val          => true
  | .iso, .box          => true
  | .trn, .ref          => true
  | .trn, .box          => true
  | .trn, .tag          => true
  | .ref, .box          => true
  | .ref, .tag          => true
  | .val, .box          => true
  | .val, .tag          => true
  | .box, .tag          => true
  | c₁, c₂              => c₁ == c₂

def join : Cap → Cap → Cap
  | .lineariso, c => c
  | c, .lineariso => c
  | .iso, .trn => .trn
  | .trn, .iso => .trn
  | .trn, .ref => .ref
  | .ref, .trn => .ref
  | .ref, .box => .box
  | .box, .ref => .box
  | .val, .box => .box
  | .box, .val => .box
  | .linear, .val => .val
  | .val, .linear => .val
  | .linear, .box => .box
  | .box, .linear => .box
  | .iso, .val => .box
  | .val, .iso => .box
  | .trn, .val => .box
  | .val, .trn => .box
  | .ref, .val => .box
  | .val, .ref => .box
  | .linear, .iso => .box
  | .iso, .linear => .box
  | _, .tag => .tag
  | .tag, _ => .tag
  | _, .box => .box
  | .box, _ => .box
  | c₁, c₂ => if c₁ == c₂ then c₁ else .tag

def isSendable : Cap → Bool
  | .iso       => true
  | .val       => true
  | .tag       => true
  | .lineariso => true
  | .linear    => true
  | _          => false

end Nulang.Capabilities
