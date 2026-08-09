Require Import String.
Require Import List.
Import ListNotations.

(** * Nulang Core Syntax *)

(** Base types *)
Inductive base_ty : Type :=
  | TInt : base_ty
  | TBool : base_ty
  | TString : base_ty
  | TUnit : base_ty.

(** Effects *)
Definition effect_name := string.
Definition op_name := string.

Inductive ty : Type :=
  | TyBase : base_ty -> ty
  | TyFun : ty -> ty -> ty.  (* Simplified for now: T1 -> T2 *)

(** Expressions *)
Definition var := string.

Inductive expr : Type :=
  | EVar : var -> expr
  | EInt : nat -> expr
  | EBool : bool -> expr
  | EString : string -> expr
  | EUnit : expr
  | EAdd : expr -> expr -> expr
  | ELam : var -> ty -> expr -> expr
  | EApp : expr -> expr -> expr
  | ELet : var -> expr -> expr -> expr
  | EIf : expr -> expr -> expr -> expr
  (* Effect operations *)
  | EPerform : effect_name -> op_name -> expr -> expr
  | EHandle : expr -> effect_name -> op_name -> var -> expr -> expr -> expr.
  (* handle e1 with { eff.op(x) => e2, resume is implicitly passed or bound? *)
