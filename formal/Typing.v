Require Import String.
Require Import List.
Import ListNotations.
Require Import Syntax.

(** * Nulang Core Typing *)

Definition env := list (var * ty).

Fixpoint lookup (x : var) (g : env) : option ty :=
  match g with
  | [] => None
  | (y, t) :: g' => if string_dec x y then Some t else lookup x g'
  end.

(** Effect signatures: maps effect name and op to (arg_ty, ret_ty) *)
Definition eff_env := list (effect_name * op_name * (ty * ty)).

Fixpoint lookup_op (eff : effect_name) (op : op_name) (sigs : eff_env) : option (ty * ty) :=
  match sigs with
  | [] => None
  | (eff', op', (t1, t2)) :: sigs' =>
      if string_dec eff eff' then
        if string_dec op op' then Some (t1, t2)
        else lookup_op eff op sigs'
      else lookup_op eff op sigs'
  end.

Inductive has_type (sigs : eff_env) : env -> expr -> ty -> Prop :=
  | T_Var : forall g x t,
      lookup x g = Some t ->
      has_type sigs g (EVar x) t
  | T_Int : forall g n,
      has_type sigs g (EInt n) (TyBase TInt)
  | T_Bool : forall g b,
      has_type sigs g (EBool b) (TyBase TBool)
  | T_String : forall g s,
      has_type sigs g (EString s) (TyBase TString)
  | T_Unit : forall g,
      has_type sigs g EUnit (TyBase TUnit)
  | T_Add : forall g e1 e2,
      has_type sigs g e1 (TyBase TInt) ->
      has_type sigs g e2 (TyBase TInt) ->
      has_type sigs g (EAdd e1 e2) (TyBase TInt)
  | T_Lam : forall g x t1 e t2,
      has_type sigs ((x, t1) :: g) e t2 ->
      has_type sigs g (ELam x t1 e) (TyFun t1 t2)
  | T_App : forall g e1 e2 t1 t2,
      has_type sigs g e1 (TyFun t1 t2) ->
      has_type sigs g e2 t1 ->
      has_type sigs g (EApp e1 e2) t2
  | T_Let : forall g x e1 e2 t1 t2,
      has_type sigs g e1 t1 ->
      has_type sigs ((x, t1) :: g) e2 t2 ->
      has_type sigs g (ELet x e1 e2) t2
  | T_If : forall g e1 e2 e3 t,
      has_type sigs g e1 (TyBase TBool) ->
      has_type sigs g e2 t ->
      has_type sigs g e3 t ->
      has_type sigs g (EIf e1 e2 e3) t
  | T_Perform : forall g eff op e arg_ty ret_ty,
      lookup_op eff op sigs = Some (arg_ty, ret_ty) ->
      has_type sigs g e arg_ty ->
      has_type sigs g (EPerform eff op e) ret_ty
  | T_Handle : forall g e1 eff op x k e2 t arg_ty ret_ty,
      has_type sigs g e1 t ->
      lookup_op eff op sigs = Some (arg_ty, ret_ty) ->
      (* In handle, e2 has x: arg_ty, and k: ret_ty -> t *)
      has_type sigs ((x, arg_ty) :: (k, TyFun ret_ty t) :: g) e2 t ->
      has_type sigs g (EHandle e1 eff op x k e2) t.
