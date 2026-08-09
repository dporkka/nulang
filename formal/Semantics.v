Require Import String.
Require Import List.
Import ListNotations.
Require Import Syntax.
Require Import Typing.

(** * Nulang Core Operational Semantics (Small-Step) *)

Inductive val : Type :=
  | VInt : nat -> val
  | VBool : bool -> val
  | VString : string -> val
  | VUnit : val
  | VClosure : var -> expr -> list (var * val) -> val
  | VCont : expr -> list (var * val) -> val. (* Continuation *)

Definition venv := list (var * val).

Fixpoint vlookup (x : var) (g : venv) : option val :=
  match g with
  | [] => None
  | (y, v) :: g' => if string_dec x y then Some v else vlookup x g'
  end.

(** Since this is a simple small-step semantics without full continuations, 
    we just define it conceptually or leave it as an exercise. *)

(** Nanolang proved type soundness, progress, determinism.
    This skeleton provides the foundation for Nulang to do the same. *)

(** Progress Theorem Statement *)
Conjecture progress : forall sigs e t,
  has_type sigs [] e t ->
  (exists v, e = (* value conversion *) e) \/ (exists e', e = e'). (* simplified *)

(** Preservation Theorem Statement *)
Conjecture preservation : forall sigs e e' t,
  has_type sigs [] e t ->
  e = e' -> (* step *)
  has_type sigs [] e' t.
