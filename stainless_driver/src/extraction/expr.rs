use super::literal::Literal;
use super::*;

use std::convert::TryFrom;

use rustc_middle::mir::{BinOp, Mutability, UnOp};
use rustc_middle::ty::TyKind;

use rustc_hair::hair::{
  Arm, BindingMode, Block, BlockSafety, Expr, ExprKind, ExprRef, FieldPat, Guard, LogicalOp,
  Mirror, Pat, PatKind, StmtKind, StmtRef,
};

use stainless_data::ast as st;

type Result<T> = std::result::Result<T, &'static str>;

/// Extraction of bodies (i.e., expressions, for the most part)
impl<'a, 'l, 'tcx> BodyExtractor<'a, 'l, 'tcx> {
  pub(super) fn extract_expr(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    match expr.kind {
      ExprKind::Literal { literal: konst, .. } => match Literal::try_from(konst) {
        Ok(lit) => lit.as_st_literal(f),
        _ => self.unsupported_expr(expr.span, "Unsupported kind of literal"),
      },
      ExprKind::Unary { .. } => self.extract_unary(expr),
      ExprKind::Binary { .. } => self.extract_binary(expr),
      ExprKind::LogicalOp { .. } => self.extract_logical_op(expr),
      ExprKind::Tuple { fields } => f.Tuple(self.extract_expr_refs(fields)).into(),
      ExprKind::Field { .. } => self.extract_field(expr),
      ExprKind::VarRef { id } => self.fetch_var(id).into(),
      ExprKind::Call { .. } => self.extract_call(expr),
      ExprKind::Adt { .. } => self.extract_adt_construction(expr),
      ExprKind::Block { body: ast_block } => {
        let block = self.mirror(ast_block);
        match block.safety_mode {
          BlockSafety::Safe => self.extract_block(block),
          _ => self.unsupported_expr(expr.span, "Cannot extract unsafe block"),
        }
      }
      ExprKind::Match {
        scrutinee,
        mut arms,
      } => {
        // TODO: Avoid this clone by just looking up the type of scrutinee for looks_like_if
        let scrutinee_ = scrutinee.clone();
        match self.looks_like_if(scrutinee, &arms) {
          Some(has_elze) => {
            let elze = arms.pop().unwrap().body;
            let then = arms.pop().unwrap().body;
            let elze_opt = if has_elze { Some(elze) } else { None };
            self.extract_if(scrutinee_, then, elze_opt)
          }
          None => self.extract_match(scrutinee_, arms),
        }
      }

      // TODO: Handle method calls
      // TODO: Handle arbitrary-precision integers
      // TODO: Handle Deref / Borrow
      ExprKind::Scope { value, .. } => self.extract_expr_ref(value),
      ExprKind::Use { source } => self.extract_expr_ref(source),
      ExprKind::NeverToAny { source } => self.extract_expr_ref(source),

      _ => self.unsupported_expr(
        expr.span,
        format!("Cannot extract expr kind {:?}", expr.kind),
      ),
    }
  }

  fn extract_expr_ref(&mut self, expr: ExprRef<'tcx>) -> st::Expr<'l> {
    let expr = self.mirror(expr);
    self.extract_expr(expr)
  }

  fn extract_expr_refs<I>(&mut self, exprs: I) -> Vec<st::Expr<'l>>
  where
    I: IntoIterator<Item = ExprRef<'tcx>>,
  {
    exprs
      .into_iter()
      .map(|arg| self.extract_expr_ref(arg))
      .collect()
  }

  fn extract_unary(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::Unary { op, arg } = expr.kind {
      let arg = self.mirror(arg);
      let arg_ty = arg.ty;
      let arg_is_bv = self.base.is_bv_type(arg_ty);
      let arg_is_int = arg_is_bv || self.base.is_bigint_type(arg_ty);
      let arg = self.extract_expr(arg);

      match op {
        UnOp::Not if arg_is_bv => f.BVNot(arg).into(),
        UnOp::Not if arg_ty.is_bool() => f.Not(arg).into(),
        UnOp::Neg if arg_is_int => f.UMinus(arg).into(),
        _ => unexpected(expr.span, format!("Cannot extract unary op {:?}", op)),
      }
    } else {
      unreachable!()
    }
  }

  fn extract_binary(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::Binary {
      op,
      lhs: arg1,
      rhs: arg2,
    } = expr.kind
    {
      let (arg1, arg2) = (self.mirror(arg1), self.mirror(arg2));
      let args_are_bv = self.base.is_bv_type(arg1.ty) && self.base.is_bv_type(arg2.ty);
      let args_are_bool = arg1.ty.is_bool() && arg2.ty.is_bool();
      assert!(args_are_bv || args_are_bool);
      let (arg1, arg2) = (self.extract_expr(arg1), self.extract_expr(arg2));
      match op {
        BinOp::Eq => f.Equals(arg1, arg2).into(),
        BinOp::Ne => f.Not(f.Equals(arg1, arg2).into()).into(),
        BinOp::Add if args_are_bv => f.Plus(arg1, arg2).into(),
        BinOp::Sub if args_are_bv => f.Minus(arg1, arg2).into(),
        BinOp::Mul if args_are_bv => f.Times(arg1, arg2).into(),
        BinOp::Div if args_are_bv => f.Division(arg1, arg2).into(),
        BinOp::Lt if args_are_bv => f.LessThan(arg1, arg2).into(),
        BinOp::Le if args_are_bv => f.LessEquals(arg1, arg2).into(),
        BinOp::Ge if args_are_bv => f.GreaterEquals(arg1, arg2).into(),
        BinOp::Gt if args_are_bv => f.GreaterThan(arg1, arg2).into(),
        _ => {
          // TODO: Handle Rem, BitXor, BitAnd, BitOr, Shl, Shr
          self.unsupported_expr(expr.span, format!("Cannot extract binary op {:?}", op))
        }
      }
    } else {
      unreachable!()
    }
  }

  fn extract_logical_op(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::LogicalOp {
      op,
      lhs: arg1,
      rhs: arg2,
    } = expr.kind
    {
      let (arg1, arg2) = (self.mirror(arg1), self.mirror(arg2));
      assert!(arg1.ty.is_bool() && arg2.ty.is_bool());
      let (arg1, arg2) = (self.extract_expr(arg1), self.extract_expr(arg2));
      match op {
        LogicalOp::And => f.And(vec![arg1, arg2]).into(),
        LogicalOp::Or => f.Or(vec![arg1, arg2]).into(),
      }
    } else {
      unreachable!()
    }
  }

  fn extract_field(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::Field { lhs, name } = expr.kind {
      let lhs = self.mirror(lhs);
      let lhs_ty = lhs.ty;
      let lhs = self.extract_expr(lhs);
      let index = name.index();
      match lhs_ty.kind {
        TyKind::Tuple(_) => f.TupleSelect(lhs, (index as i32) + 1).into(),
        TyKind::Adt(adt_def, _) => {
          let sort = self.base.extract_adt(adt_def.did);
          assert_eq!(sort.constructors.len(), 1);
          let constructor = sort.constructors[0];
          assert!(index < constructor.fields.len());
          f.ADTSelector(lhs, constructor.fields[index].v.id).into()
        }
        _ => unexpected(expr.span, "Unexpected kind of field selection"),
      }
    } else {
      unreachable!()
    }
  }

  fn extract_call(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::Call { ty, args, .. } = expr.kind {
      if let TyKind::FnDef(def_id, _substs_ref) = ty.kind {
        let args = self.extract_expr_refs(args);
        let fd_id = self.base.extract_fn_ref(def_id);
        // TODO: Also consider type arguments
        f.FunctionInvocation(fd_id, vec![], args).into()
      } else {
        self.unsupported_expr(
          expr.span,
          "Cannot extract call without statically known target",
        )
      }
    } else {
      unreachable!()
    }
  }

  /*
  // Expressions for which `e.clone()` can be translated simply as `e`.
  // This is sound, in particular, for types for which we don't extract any
  // mutating operations.
  fn can_treat_clone_as_identity(&mut self, expr: &'tcx hir::Expr<'tcx>) -> bool {
    let expr_ty = self.tables.node_type(expr.hir_id);
    match expr_ty.kind {
      TyKind::Adt(adt_def, _) => self.base.is_bigint(adt_def),
      _ => false,
    }
  }

  fn extract_conversion_into(
    &mut self,
    outer: &'tcx hir::Expr<'tcx>,
    inner: &'tcx hir::Expr<'tcx>,
  ) -> st::Expr<'l> {
    let from_ty = self.tables.node_type(inner.hir_id);
    let to_ty = self.tables.node_type(outer.hir_id);
    match (&from_ty.kind, &to_ty.kind) {
      (TyKind::Int(_), TyKind::Adt(adt_def, _)) if self.base.is_bigint(adt_def) => self
        .try_extract_bigint_lit(inner)
        .unwrap_or_else(|reason| self.unsupported_expr(inner, reason)),
      _ => self.unsupported_expr(
        outer,
        format!("Cannot extract conversion from {} to {}", from_ty, to_ty),
      ),
    }
  }

  fn try_extract_bigint_lit(&mut self, expr: &'tcx hir::Expr<'tcx>) -> Result<'l> {
    use ast::LitKind;
    let f = self.factory();
    if let ExprKind::Lit(ref lit) = expr.kind {
      match lit.node {
        LitKind::Int(value, _) => {
          let node_ty = self.tables.node_type(expr.hir_id);
          match node_ty.kind {
            ty::Int(_) => Ok(f.IntegerLiteral((value as i128).into()).into()),
            _ => Err("Cannot extract BigInt from non-signed-int literal"),
          }
        }
        _ => Err("Cannot extract BigInt from non-integral literal kind"),
      }
    } else {
      Err("Can only extract BigInt from integer literals")
    }
  }

  fn try_extract_bigint_expr(&mut self, expr: Expr<'tcx>) -> Result<'l> {
    self.try_extract_bigint_lit(expr).or_else(|_| {
      let expr_ty = self.tables.node_type(expr.hir_id);
      if self.base.is_bigint_type(expr_ty) {
        Ok(self.extract_expr(expr))
      } else {
        Err("Not a BigInt-convertible expr")
      }
    })
  }

  fn extract_method_call(&mut self, expr: &'tcx hir::Expr<'tcx>) -> st::Expr<'l> {
    if let ExprKind::MethodCall(_path_seg, _, args) = expr.kind {
      let def_path = self
        .tables
        .type_dependent_def(expr.hir_id)
        .map(|(_, def_id)| def_id)
        .map(|def_id| self.tcx().def_path_str(def_id))
        .unwrap_or_else(|| "<unknown>".into());
      let arg = &args[0];
      // TODO: Fast check using `path_seg.ident.name == Symbol::intern("into")`?
      match def_path.as_str() {
        "std::convert::Into::into" => self.extract_conversion_into(expr, arg),
        "std::clone::Clone::clone" if self.can_treat_clone_as_identity(expr) => {
          self.extract_expr(arg)
        }
        _ => self.unsupported_expr(expr, "Cannot extract general method calls"),
      }
    } else {
      unreachable!()
    }
  }
  */

  fn extract_adt_construction(&mut self, expr: Expr<'tcx>) -> st::Expr<'l> {
    let f = self.factory();
    if let ExprKind::Adt {
      adt_def,
      variant_index,
      mut fields,
      base,
      ..
    } = expr.kind
    {
      if base.is_some() {
        self.unsupported_expr(expr.span, "Cannot extract ADT constructions with bases")
      } else {
        // TODO: Also consider type arguments
        let sort = self.base.extract_adt(adt_def.did);
        let constructor = sort.constructors[variant_index.index()];
        fields.sort_by_key(|field| field.name.index());
        let args = fields
          .into_iter()
          .map(|field| self.extract_expr_ref(field.expr))
          .collect();
        f.ADT(constructor.id, vec![], args).into()
      }
    } else {
      unreachable!()
    }
  }

  fn extract_if(
    &mut self,
    cond: ExprRef<'tcx>,
    then: ExprRef<'tcx>,
    elze_opt: Option<ExprRef<'tcx>>,
  ) -> st::Expr<'l> {
    let f = self.factory();
    let cond = self.extract_expr_ref(cond);
    let then = self.extract_expr_ref(then);
    let elze = elze_opt
      .map(|e| self.extract_expr_ref(e))
      .unwrap_or_else(|| {
        // TODO: Match the type of the then branch?
        f.UnitLiteral().into()
      });
    f.IfExpr(cond, then, elze).into()
  }

  fn extract_match(&mut self, scrutinee: ExprRef<'tcx>, arms: Vec<Arm<'tcx>>) -> st::Expr<'l> {
    let scrutinee = self.extract_expr_ref(scrutinee);
    let cases = arms.into_iter().map(|arm| self.extract_arm(arm)).collect();
    self.factory().MatchExpr(scrutinee, cases).into()
  }

  fn extract_arm(&mut self, arm: Arm<'tcx>) -> &'l st::MatchCase<'l> {
    let Arm {
      pattern,
      guard,
      body,
      ..
    } = arm;
    let pattern = self.extract_pattern(pattern, None);
    let guard = guard.map(|Guard::If(expr)| self.extract_expr_ref(expr));
    let body = self.extract_expr_ref(body);
    self.factory().MatchCase(pattern, guard, body)
  }

  fn extract_pattern(
    &mut self,
    pattern: Pat<'tcx>,
    binder: Option<&'l st::ValDef<'l>>,
  ) -> st::Pattern<'l> {
    let f = self.factory();
    match pattern.kind {
      box PatKind::Wild => f.WildcardPattern(binder).into(),
      box kind @ PatKind::Binding { .. } => {
        assert!(binder.is_none());
        match self.try_pattern_to_var(&kind, true) {
          Ok(binder) => {
            let binder = f.ValDef(binder);
            match kind {
              PatKind::Binding {
                subpattern: Some(subpattern),
                ..
              } => self.extract_pattern(subpattern, Some(binder)),
              PatKind::Binding {
                subpattern: None, ..
              } => f.WildcardPattern(Some(binder)).into(),
              _ => unreachable!(),
            }
          }
          Err(reason) => self.unsupported_pattern(
            pattern.span,
            format!("Unsupported pattern binding: {}", reason),
          ),
        }
      }
      box PatKind::Variant {
        adt_def,
        variant_index,
        subpatterns,
        ..
      } => {
        // TODO: Also consider type arguments
        let sort = self.base.extract_adt(adt_def.did);
        let constructor = sort.constructors[variant_index.index()];
        let subpatterns = self.extract_subpatterns(subpatterns, constructor.fields.len());
        f.ADTPattern(binder, constructor.id, vec![], subpatterns)
          .into()
      }
      box PatKind::Leaf { subpatterns } => {
        // TODO: Also consider type arguments
        if let TyKind::Adt(adt_def, _) = pattern.ty.kind {
          let sort = self.base.extract_adt(adt_def.did);
          assert_eq!(sort.constructors.len(), 1);
          let constructor = sort.constructors[0];
          let subpatterns = self.extract_subpatterns(subpatterns, constructor.fields.len());
          f.ADTPattern(binder, constructor.id, vec![], subpatterns)
            .into()
        } else {
          unexpected(
            pattern.span,
            "Encountered Leaf pattern, but type is not an ADT",
          );
        }
      }
      box PatKind::Constant { value: konst } => match Literal::try_from(konst) {
        Ok(lit) => f.LiteralPattern(binder, lit.as_st_literal(f)).into(),
        _ => self.unsupported_pattern(pattern.span, "Unsupported kind of literal in pattern"),
      },
      _ => self.unsupported_pattern(pattern.span, "Unsupported kind of pattern"),
    }
  }

  fn extract_subpatterns(
    &mut self,
    mut field_pats: Vec<FieldPat<'tcx>>,
    num_fields: usize,
  ) -> Vec<st::Pattern<'l>> {
    let f = self.factory();
    field_pats.sort_by_key(|field| field.field.index());
    field_pats.reverse();
    let mut subpatterns = Vec::with_capacity(num_fields);
    for i in 0..num_fields {
      let next = if let Some(FieldPat { field, .. }) = field_pats.last() {
        if field.index() == i {
          let FieldPat { pattern, .. } = field_pats.pop().unwrap();
          self.extract_pattern(pattern, None)
        } else {
          f.WildcardPattern(None).into()
        }
      } else {
        f.WildcardPattern(None).into()
      };
      subpatterns.push(next);
    }
    subpatterns
  }

  #[allow(clippy::unnecessary_unwrap)]
  fn extract_block_(
    &mut self,
    stmts: &mut Vec<StmtRef<'tcx>>,
    acc_exprs: &mut Vec<st::Expr<'l>>,
    final_expr: st::Expr<'l>,
  ) -> st::Expr<'l> {
    let f = self.factory();
    let finish = |exprs: Vec<st::Expr<'l>>, final_expr| {
      if exprs.is_empty() {
        final_expr
      } else {
        f.Block(exprs, final_expr).into()
      }
    };

    if let Some(stmt) = stmts.pop() {
      let stmt = self.mirror(stmt);
      match stmt.kind {
        StmtKind::Let {
          pattern,
          initializer,
          ..
        } => {
          let span = pattern.span;
          let bail = |this: &mut Self, msg| -> st::Expr<'l> {
            this.base.unsupported(span, msg);
            f.Block(acc_exprs.clone(), f.NoTree(f.Untyped().into()).into())
              .into()
          };
          // FIXME: Detect desugared `let`s
          let has_abnormal_source = false;
          let var_result = self.try_pattern_to_var(&pattern.kind, false);

          if has_abnormal_source {
            // TODO: Support for loops
            bail(self, "Cannot extract let that resulted from desugaring")
          } else if let Err(reason) = var_result {
            // TODO: Desugar complex patterns
            bail(
              self,
              format!("Cannot extract complex pattern in let: {}", reason).as_str(),
            )
          } else if initializer.is_none() {
            bail(self, "Cannot extract let without initializer")
          } else {
            let vd = f.ValDef(var_result.unwrap());
            let init = self.extract_expr_ref(initializer.unwrap());
            let exprs = acc_exprs.clone();
            acc_exprs.clear();
            let body_expr = self.extract_block_(stmts, acc_exprs, final_expr);
            let last_expr = f.Let(vd, init, body_expr).into();
            finish(exprs, last_expr)
          }
        }
        StmtKind::Expr { expr, .. } => {
          let expr = self.extract_expr_ref(expr);
          acc_exprs.push(expr);
          self.extract_block_(stmts, acc_exprs, final_expr)
        }
      }
    } else {
      finish(acc_exprs.clone(), final_expr)
    }
  }

  fn extract_block(&mut self, block: Block<'tcx>) -> st::Expr<'l> {
    let Block {
      mut stmts,
      expr: final_expr,
      ..
    } = block;
    let final_expr = final_expr
      .map(|e| self.extract_expr_ref(e))
      .unwrap_or_else(|| self.factory().UnitLiteral().into());
    stmts.reverse();
    self.extract_block_(&mut stmts, &mut vec![], final_expr)
  }

  /// Various helpers

  fn mirror<M: Mirror<'tcx>>(&mut self, m: M) -> M::Output {
    m.make_mirror(&mut self.hcx)
  }

  fn strip_scopes(&mut self, expr: Expr<'tcx>) -> Expr<'tcx> {
    match expr.kind {
      ExprKind::Scope { value, .. } => {
        let expr = self.mirror(value);
        self.strip_scopes(expr)
      }
      _ => expr,
    }
  }

  /// Try to detect whether the given match corresponds to an if expression.
  /// Returns None if it is not an if expression and Some(has_elze) otherwise.
  fn looks_like_if(&mut self, scrutinee: ExprRef<'tcx>, arms: &[Arm<'tcx>]) -> Option<bool> {
    let cond = self.mirror(scrutinee);
    let is_if = arms.len() == 2
      && cond.ty.is_bool()
      && match (&arms[0].pattern.kind, &arms[1].pattern.kind) {
        (box PatKind::Constant { value: konst }, box PatKind::Wild) => {
          match Literal::try_from(*konst) {
            Ok(Literal::Bool(true)) => true,
            _ => false,
          }
        }
        _ => false,
      };

    if is_if {
      let elze = self.mirror(arms[1].body.clone());
      match self.strip_scopes(elze).kind {
        ExprKind::Block { body: ast_block } => {
          let Block { stmts, expr, .. } = self.mirror(ast_block);
          let elze_missing = stmts.is_empty() && expr.is_none();
          Some(!elze_missing)
        }
        _ => unreachable!(),
      }
    } else {
      None
    }
  }

  fn try_pattern_to_var(
    &mut self,
    pat_kind: &PatKind<'tcx>,
    allow_subpattern: bool,
  ) -> Result<&'l st::Variable<'l>> {
    match pat_kind {
      PatKind::Binding {
        mutability,
        mode,
        var: hir_id,
        subpattern,
        ..
      } => {
        let is_by_value = if let BindingMode::ByValue = mode {
          true
        } else {
          false
        };
        if *mutability != Mutability::Not {
          Err("Mutable bindings are not supported")
        } else if !is_by_value {
          Err("By-reference bindings are supported")
        } else if !allow_subpattern && subpattern.is_some() {
          Err("Subpatterns are not supported here")
        } else {
          Ok(self.fetch_var(*hir_id))
        }
      }
      _ => Err("Expected a top-level binding"),
    }
  }

  fn unsupported_expr<M: Into<String>>(&mut self, span: Span, msg: M) -> st::Expr<'l> {
    self.base.unsupported(span, msg);
    let f = self.factory();
    f.NoTree(f.Untyped().into()).into()
  }

  fn unsupported_pattern<M: Into<String>>(&mut self, span: Span, msg: M) -> st::Pattern<'l> {
    self.base.unsupported(span, msg);
    self.factory().WildcardPattern(None).into()
  }
}