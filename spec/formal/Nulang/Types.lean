namespace Nulang.Types

abbrev TypeVar := Nat

inductive Ty where
  | var  (v : TypeVar)
  | int | bool | string | unit | nil
  | fn   (param : Ty) (ret : Ty)
  | tuple (ts : List Ty)
  deriving BEq, Repr, Inhabited

mutual
  def freeVars : Ty → List TypeVar
    | .var v    => [v]
    | .fn p r   => freeVars p ++ freeVars r
    | .tuple ts => freeVarsList ts
    | _         => []
  termination_by structural t => t

  def freeVarsList : List Ty → List TypeVar
    | []      => []
    | t :: ts => freeVars t ++ freeVarsList ts
  termination_by structural ts => ts
end

abbrev Subst := List (TypeVar × Ty)
def substEmpty : Subst := []

def substApplyVar (σ : Subst) (v : TypeVar) : Ty :=
  match σ.lookup v with
  | some t => t
  | none   => .var v

mutual
  def substApply (σ : Subst) : Ty → Ty
    | .var v    => substApplyVar σ v
    | .fn p r   => .fn (substApply σ p) (substApply σ r)
    | .tuple ts => .tuple (substApplyList σ ts)
    | t         => t
  termination_by structural t => t

  def substApplyList (σ : Subst) : List Ty → List Ty
    | []      => []
    | t :: ts => substApply σ t :: substApplyList σ ts
  termination_by structural ts => ts
end

def substCompose (σ₂ : Subst) (σ₁ : Subst) : Subst :=
  let applied := σ₁.map (λ (v, t) => (v, substApply σ₂ t))
  applied ++ σ₂

mutual
  def occurs (v : TypeVar) : Ty → Bool
    | .var w    => v == w
    | .fn p r   => occurs v p || occurs v r
    | .tuple ts => occursList v ts
    | _         => false
  termination_by structural t => t

  def occursList (v : TypeVar) : List Ty → Bool
    | []      => false
    | t :: ts => occurs v t || occursList v ts
  termination_by structural ts => ts
end

partial def mgu : Ty → Ty → Option Subst
  | .var v, .var w =>
    if v == w then some substEmpty
    else some [(v, .var w)]
  | .var v, t =>
    if occurs v t then none
    else some [(v, t)]
  | t, .var v =>
    if occurs v t then none
    else some [(v, t)]
  | .int, .int => some substEmpty
  | .bool, .bool => some substEmpty
  | .string, .string => some substEmpty
  | .unit, .unit => some substEmpty
  | .nil, .nil => some substEmpty
  | .fn p₁ r₁, .fn p₂ r₂ =>
    match mgu p₁ p₂ with
    | none => none
    | some σ₁ =>
      match mgu (substApply σ₁ r₁) (substApply σ₁ r₂) with
      | none => none
      | some σ₂ => some (substCompose σ₂ σ₁)
  | .tuple ts₁, .tuple ts₂ =>
    if ts₁.length != ts₂.length then none
    else mguTuple ts₁ ts₂ substEmpty
  | _, _ => none
where
  mguTuple : List Ty → List Ty → Subst → Option Subst
    | [], [], σ => some σ
    | t₁::ts₁, t₂::ts₂, σ =>
      match mgu (substApply σ t₁) (substApply σ t₂) with
      | none => none
      | some σ' => mguTuple ts₁ ts₂ (substCompose σ' σ)
    | _, _, _ => none

end Nulang.Types
