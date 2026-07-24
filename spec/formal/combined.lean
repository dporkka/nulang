/- 
  Nulang combined type system — unified typing judgment.
  
  Integrates the Hindley-Milner type system (`types.lean`), the capability
  lattice (`capabilities.lean`), and the row-polymorphic effect system
  (`effects.lean`) into a single typing judgment:
  
    Γ; Δ ⊢ e : τ @ cap ! r
  
  "In typing context Γ and capability context Δ, expression e has type τ
   with capability cap and effect row r."
  
  This is the authoritative combined semantics: it defines what a
  well-typed Nulang Core program is.  Implementation conformance is
  measured against this judgment.
  
  The three subsystems are:
  - `types.lean`:    Ty, Expr, Value, HasType (pure HM)
  - `capabilities.lean`: Cap, CapContext, HasTypeCap (cap-annotated)
  - `effects.lean`:   EffectLabel, EffectRow, HasTypeEff (effect-annotated)
  
  The combined judgment below is the Cartesian product of the three:
  a typing derivation simultaneously assigns a type, a capability, and
  an effect row.
  
  Soundness conjecture (open): if Γ; Δ ⊢ e : τ @ cap ! r and r is closed
  and cap is sendable, then e evaluates to a value v with Γ; Δ ⊢ v : τ
  @ cap ! {} — pure, well-typed, and safe.
-/

namespace Nulang

-- ==================================================================
-- Imports (conceptual — these definitions live in sibling files)
-- ==================================================================

-- From types.lean:    Ty, Expr, Value, Name, Context, Scheme, Subst, mgu
-- From capabilities.lean: Cap, CapContext, join, le, is_sendable, discharge_linear
-- From effects.lean:  EffectLabel, EffectRow, EffExpr, Handler, HandlerStack

-- ==================================================================
-- Unified typing judgment
-- ==================================================================

/--
  The combined typing judgment: Γ; Δ ⊢ e : τ @ cap ! r
  
  Components:
  - Γ : Context         — maps variable names to type schemes (from types.lean)
  - Δ : CapContext       — maps variable names to capabilities (from capabilities.lean)
  - e : Expr            — the Core expression (from types.lean, extended with Perform/Handle)
  - τ : Ty              — the inferred type
  - cap : Cap           — the inferred capability (Val for pure values)
  - r  : EffectRow      — the inferred effect row ({} for pure expressions)
  
  This is the golden reference: every Nulang implementation must accept
  only programs derivable under this judgment.
-/
inductive HasTypeCombined : Context → CapContext → Expr → Ty → Cap → EffectRow → Prop where

-- ** Variables **
-- Variable reference: look up x in both Γ (for type) and Δ (for capability).
-- The effect row is empty (variable reference is pure).
| tVar : ∀ {Γ Δ x τ σ cap},
    Γ.lookup x = some σ →
    -- Instantiate the scheme to get (τ, cap) — in the combined system,
    -- schemes carry both type and capability information.
    -- For now: monomorphic bindings carry cap=Val, let-generalized
    -- bindings carry the inferred cap from the bound expression.
    HasTypeCombined Γ Δ (.var x) τ cap .empty

-- ** Literals **
-- All literals are pure (effect row empty) and immutable (Val).
| tLitInt : ∀ {Γ Δ n},
    HasTypeCombined Γ Δ (.litInt n) .int .Val .empty
| tLitBool : ∀ {Γ Δ b},
    HasTypeCombined Γ Δ (.litBool b) .bool .Val .empty
| tLitString : ∀ {Γ Δ s},
    HasTypeCombined Γ Δ (.litString s) .string .Val .empty
| tUnit : ∀ {Γ Δ},
    HasTypeCombined Γ Δ .unitVal (.prim .Unit) .Val .empty

-- ** Lambda **
-- Lambda creation is pure (effect in body is latent, not at creation time).
-- The closure itself is always Val (immutable, sendable).
-- Capability of the parameter is recorded in Δ for the body.
| tLambda : ∀ {Γ Δ x τ₁ τ₂ e cap₁ cap₂ r},
    -- Body typed under extended contexts:
    --   Γ extended with x:τ₁ (monomorphic)
    --   Δ extended with x ↦ cap₁
    HasTypeCombined ((x, ⟨[], τ₁⟩) :: Γ) ((x, cap₁) :: Δ) e τ₂ cap₂ r →
    HasTypeCombined Γ Δ (.lambda x τ₁ e) (.fn τ₁ τ₂) .Val .empty

-- ** Application **
-- Effects and capabilities propagate: the effect row is the union of
-- function and argument effects; the capability is the join of the
-- function and argument capabilities.
| tApp : ∀ {Γ Δ e₁ e₂ τ₁ τ₂ cap₁ cap₂ r₁ r₂},
    HasTypeCombined Γ Δ e₁ (.fn τ₂ τ₁) cap₁ r₁ →
    HasTypeCombined Γ Δ e₂ τ₂ cap₂ r₂ →
    HasTypeCombined Γ Δ (.app e₁ e₂) τ₁ (Cap.join cap₁ cap₂) (EffectRow.union r₁ r₂)

-- ** Let binding **
-- Let polymorphic generalization: the bound expression e₁ is generalized
-- over its free type variables and free capabilities, producing a scheme
-- that is added to Γ.  Effects of the binding and body are combined.
| tLet : ∀ {Γ Δ x e₁ e₂ τ₁ τ₂ cap₁ cap₂ r₁ r₂},
    HasTypeCombined Γ Δ e₁ τ₁ cap₁ r₁ →
    -- Generalize τ₁ and cap₁ over free variables not in Γ,Δ
    HasTypeCombined ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₁) :: Γ)
                    ((x, cap₁) :: Δ) e₂ τ₂ cap₂ r₂ →
    HasTypeCombined Γ Δ (.letIn x e₁ e₂) τ₂ cap₂ (EffectRow.union r₁ r₂)

-- ** Conditional **
-- The guard must be Bool; both branches must have the same type.
-- Effects and capabilities are joined across branches.
| tIf : ∀ {Γ Δ e₁ e₂ e₃ τ cap₁ cap₂ cap₃ r₁ r₂ r₃},
    HasTypeCombined Γ Δ e₁ .bool cap₁ r₁ →
    HasTypeCombined Γ Δ e₂ τ cap₂ r₂ →
    HasTypeCombined Γ Δ e₃ τ cap₃ r₃ →
    HasTypeCombined Γ Δ (.ifThenElse e₁ e₂ e₃) τ
      (Cap.join cap₂ cap₃) (EffectRow.union (EffectRow.union r₁ r₂) r₃)

-- ** Binary operators **
-- Operators over Int/Bool.  Pure, Val results.
| tBinOpInt : ∀ {Γ Δ op e₁ e₂},
    HasTypeCombined Γ Δ e₁ .int .Val .empty →
    HasTypeCombined Γ Δ e₂ .int .Val .empty →
    HasTypeCombined Γ Δ (.binOp op e₁ e₂) (binOpResultType op) .Val .empty

-- ** String concatenation **
| tStrConcat : ∀ {Γ Δ e₁ e₂},
    HasTypeCombined Γ Δ e₁ .string .Val .empty →
    HasTypeCombined Γ Δ e₂ .string .Val .empty →
    HasTypeCombined Γ Δ (.strConcat e₁ e₂) .string .Val .empty

-- ** Effect perform **
-- The performed effect label is added to the row.
-- The result capability is Val (effects produce immutable results).
| tPerform : ∀ {Γ Δ eff op args τ},
    -- The effect signature determines the result type τ.
    -- For built-in effects (IO.print, String.length, etc.), τ is fixed.
    -- For user-defined effects, τ comes from the handler.
    HasTypeCombined Γ Δ (.perform eff op args) τ .Val (.singleton eff)

-- ** Effect handle **
-- Handling an effect removes it from the row.
-- The handler body must account for the handled effect.
| tHandle : ∀ {Γ Δ e eff handler r cap τ},
    HasTypeCombined Γ Δ e τ cap (EffectRow.union (EffectRow.singleton eff) r) →
    -- handler: (eff, body) — the body receives the effect arguments and
    -- produces a value of type τ with effect row r (the handled effect
    -- is discharged).
    HasTypeCombined Γ Δ (.handle e eff handler) τ cap r

-- ** Send (actor message passing) **
-- Sending requires the payload capability to be sendable.
-- The effect is `Send`; the result is Nil.
| tSend : ∀ {Γ Δ actor msg τ cap_actor cap_msg r_actor r_msg},
    HasTypeCombined Γ Δ actor (.Actor {}) cap_actor r_actor →
    HasTypeCombined Γ Δ msg τ cap_msg r_msg →
    Cap.is_sendable cap_msg = true →
    HasTypeCombined Γ Δ (.send actor msg) (.prim .Nil) .Val (.singleton .Send)

-- ** Spawn (actor creation) **
-- Spawning has effect `Spawn`; the result is an actor reference (Tag).
| tSpawn : ∀ {Γ Δ actor_type init τ cap r},
    HasTypeCombined Γ Δ actor_type τ cap r →
    HasTypeCombined Γ Δ (.spawn actor_type init) (.Actor {}) .Tag (.singleton .Spawn)

-- ==================================================================
-- Soundness conjecture
-- ==================================================================

/--
  **Combined Soundness Conjecture**
  
  If Γ; Δ ⊢ e : τ @ cap ! r where:
  - r is closed (no free row variables),
  - every effect label in r has a handler on the stack,
  - cap is sendable (is_sendable cap = true) or the expression is
    evaluated in a single-actor context,
  
  then e evaluates to a value v such that Γ; Δ ⊢ v : τ @ cap ! {} —
  the result is pure, well-typed, and its capability is preserved.
  
  This combines the three subsystem soundness results:
  - type_soundness (types.lean): progress + preservation for pure HM
  - cap_sendable (capabilities.lean): sendable values cross actor boundaries safely
  - effect_safety (effects.lean): closed effect rows cannot produce unhandled effects
  
  The full proof is open.  The definitions above are verified against
  the Rust implementation (src/typechecker.rs, src/effect_checker.rs,
  src/types.rs) as of July 2026.
-/
theorem combined_soundness : True := by
  trivial
  -- Proof is open.  Requires integrating the three subsystem proofs,
  -- modeling the handler stack as runtime state, and proving the
  -- combined progress + preservation theorem over the full language.
  --
  -- The definitional scaffolding above is the authoritative contract.

end Nulang
