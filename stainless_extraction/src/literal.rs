use std::convert::TryFrom;

use rustc_middle::mir::interpret::ConstValue;
use rustc_middle::ty::{self, ConstKind, TyKind};
use rustc_target::abi;

use stainless_data::ast as st;
use stainless_data::ser::types as st_types;

pub(super) enum Literal {
  Unit,
  Bool(bool),
  Int { value: i128, size: u64 },
  Uint { value: u128, size: u64 },
  String(String),
}

impl Literal {
  pub(super) fn as_st_literal<'l>(&self, f: &'l st::Factory) -> st::Expr<'l> {
    match self {
      Literal::Unit => f.UnitLiteral().into(),
      Literal::Bool(value) => f.BooleanLiteral(*value).into(),
      // TODO: Double-check that this corresponds to Stainless' decoding of values
      Literal::Int { value, size } => f
        .BVLiteral(true, (*value).into(), *size as st_types::Int)
        .into(),
      Literal::Uint { value, size } => f
        .BVLiteral(false, (*value).into(), *size as st_types::Int)
        .into(),
      Literal::String(value) => f.StringLiteral(value.clone()).into(),
    }
  }
}

impl<'tcx> TryFrom<&'tcx ty::Const<'tcx>> for Literal {
  type Error = ();

  fn try_from(konst: &'tcx ty::Const<'tcx>) -> Result<Self, Self::Error> {
    match konst.ty.kind {
      _ if konst.ty.is_unit() => Ok(Literal::Unit),
      TyKind::Bool => {
        let value = konst.val.try_to_bits(abi::Size::from_bits(1)).unwrap() == 1;
        Ok(Literal::Bool(value))
      }
      // TODO: Handle `isize` and `usize`
      TyKind::Int(int_ty) => int_ty
        .bit_width()
        .and_then(|size| {
          let value = konst.val.try_to_bits(abi::Size::from_bits(size));
          value.map(|value| Literal::Int {
            value: value as i128,
            size,
          })
        })
        .ok_or(()),
      TyKind::Uint(uint_ty) => uint_ty
        .bit_width()
        .and_then(|size| {
          let value = konst.val.try_to_bits(abi::Size::from_bits(size));
          value.map(|value| Literal::Uint { value, size })
        })
        .ok_or(()),
      TyKind::Ref(
        _,
        ty::TyS {
          kind: TyKind::Str, ..
        },
        _,
      ) => match konst.val {
        ConstKind::Value(ConstValue::Slice { data, start, end }) => {
          let slice = data.inspect_with_undef_and_ptr_outside_interpreter(start..end);
          let s = ::std::str::from_utf8(slice).expect("Expected UTF8 str in ConstValue");
          Ok(Literal::String(s.into()))
        }
        _ => Err(()),
      },
      _ => Err(()),
    }
  }
}
