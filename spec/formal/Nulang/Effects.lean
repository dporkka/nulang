namespace Nulang.Effects

inductive EffectName where
  | io | spawn | send | receive | timer | signal | inference | provider
  deriving BEq, Repr, Inhabited

abbrev Region := Nat

inductive EffectRow where
  | closed (effects : List EffectName)
  | open   (effects : List EffectName) (region : Region)
  deriving BEq, Repr, Inhabited

def subrow : EffectRow → EffectRow → Bool
  | .closed es₁, .closed es₂ => es₁.all (λ e₁ => es₂.elem e₁)
  | .open es₁ _, .closed es₂ => es₁.all (λ e₁ => es₂.elem e₁)
  | .closed es₁, .open es₂ _ => es₁.all (λ e₁ => es₂.elem e₁)
  | .open es₁ _, .open es₂ _ => es₁.all (λ e₁ => es₂.elem e₁)

def eraseDups {α : Type} [BEq α] : List α → List α
  | [] => []
  | x :: xs =>
    let rest := eraseDups xs
    if rest.elem x then rest else x :: rest

def rowUnion (fresh : Region) : EffectRow → EffectRow → EffectRow
  | .closed es₁, .closed es₂ =>
    .closed (eraseDups (es₁ ++ es₂))
  | .open es₁ _, .closed es₂ =>
    .open (eraseDups (es₁ ++ es₂)) fresh
  | .closed es₁, .open es₂ _ =>
    .open (eraseDups (es₁ ++ es₂)) fresh
  | .open es₁ _, .open es₂ _ =>
    .open (eraseDups (es₁ ++ es₂)) fresh

end Nulang.Effects
