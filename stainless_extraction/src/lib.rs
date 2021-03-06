#![feature(rustc_private)]
#![feature(box_patterns)]

#[macro_use]
extern crate lazy_static;

extern crate rustc_ast;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_hir_pretty;
extern crate rustc_infer;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_target;
extern crate rustc_ty;

mod bindings;
mod expr;
mod flags;
mod krate;
mod literal;
mod spec;
mod std_items;
mod ty;
mod utils;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use rustc_hair::hair;
use rustc_hir::def_id::DefId;
use rustc_hir::intravisit::{self, NestedVisitorMap, Visitor};
use rustc_hir::{self as hir, HirId};
use rustc_infer::infer::{InferCtxt, TyCtxtInferExt};
use rustc_middle::span_bug;
use rustc_middle::ty::{TyCtxt, TypeckTables};
use rustc_span::{MultiSpan, Span};

use stainless_data::ast as st;

use bindings::DefContext;
use std_items::StdItems;
use ty::TyExtractionCtxt;
use utils::UniqueCounter;

/// The entrypoint into extraction
pub fn extract_crate<'l, 'tcx: 'l>(
  tcx: TyCtxt<'tcx>,
  factory: &'l st::Factory,
  crate_name: String,
) -> st::Symbols<'l> {
  let extraction = Box::new(Extraction::new(factory));
  let std_items = Rc::new(StdItems::collect(tcx));
  let mut xtor = BaseExtractor::new(tcx, std_items, extraction);
  xtor.process_crate(crate_name);

  let (adts, functions) = xtor.into_result();

  // Output extracted Stainless program
  eprintln!("[ Extracted ADTs and functions ]");
  for adt in &adts {
    eprintln!(" - ADT {}", adt.id);
    // eprintln!(" > {:#?}", adt);
  }
  for fd in &functions {
    eprintln!(" - Fun {}", fd.id);
    // eprintln!(" > {:#?}", fd);
  }
  eprintln!();

  st::Symbols::new(adts, functions)
}

/// Helpful type aliases
type StainlessSymId<'l> = &'l st::SymbolIdentifier<'l>;
type Params<'l> = Vec<&'l st::ValDef<'l>>;

/// A mapping between Rust ids and Stainless ids
struct SymbolMapping<'l> {
  global_id_counter: UniqueCounter<()>,
  local_id_counter: UniqueCounter<String>,
  did_to_stid: HashMap<DefId, StainlessSymId<'l>>,
  hid_to_stid: HashMap<HirId, StainlessSymId<'l>>,
}

/// Extraction encapsulates the state of extracting a Stainless program
struct Extraction<'l> {
  mapping: SymbolMapping<'l>,
  factory: &'l st::Factory,
  adts: HashMap<StainlessSymId<'l>, &'l st::ADTSort<'l>>,
  function_refs: HashSet<DefId>,
  functions: HashMap<StainlessSymId<'l>, &'l st::FunDef<'l>>,
}

impl<'l> Extraction<'l> {
  fn new(factory: &'l st::Factory) -> Self {
    Self {
      mapping: SymbolMapping {
        global_id_counter: UniqueCounter::new(),
        local_id_counter: UniqueCounter::new(),
        did_to_stid: HashMap::new(),
        hid_to_stid: HashMap::new(),
      },
      factory,
      adts: HashMap::new(),
      function_refs: HashSet::new(),
      functions: HashMap::new(),
    }
  }

  fn fresh_id(&mut self, name: String, symbol_path: Vec<String>) -> StainlessSymId<'l> {
    let global_id = self.mapping.global_id_counter.fresh(&());
    let local_id = self.mapping.local_id_counter.fresh(&name);
    let id = self.factory.Identifier(name, global_id, local_id);
    self.factory.SymbolIdentifier(id, symbol_path)
  }
}

/// Extractor combines rustc state with extraction state
struct BaseExtractor<'l, 'tcx: 'l> {
  tcx: TyCtxt<'tcx>,
  std_items: Rc<StdItems>,
  extraction: Option<Box<Extraction<'l>>>,
}

impl<'l, 'tcx> BaseExtractor<'l, 'tcx> {
  fn new(tcx: TyCtxt<'tcx>, std_items: Rc<StdItems>, extraction: Box<Extraction<'l>>) -> Self {
    Self {
      tcx,
      std_items,
      extraction: Some(extraction),
    }
  }

  fn into_result(self) -> (Vec<&'l st::ADTSort<'l>>, Vec<&'l st::FunDef<'l>>) {
    self.with_extraction(|xt| {
      let adts: Vec<&st::ADTSort> = xt.adts.values().copied().collect();
      let functions: Vec<&st::FunDef> = xt.functions.values().copied().collect();
      (adts, functions)
    })
  }

  #[inline]
  fn with_extraction<T, F: FnOnce(&Extraction<'l>) -> T>(&self, f: F) -> T {
    f(&**self.extraction.as_ref().expect("BodyExtractor active"))
  }

  #[inline]
  fn with_extraction_mut<T, F: FnOnce(&mut Extraction<'l>) -> T>(&mut self, f: F) -> T {
    f(self.extraction.as_mut().expect("BodyExtractor active"))
  }

  #[inline]
  fn factory(&self) -> &'l st::Factory {
    self.with_extraction(|xt| xt.factory)
  }

  /// Identifier mappings

  fn fresh_id(&mut self, name: String) -> StainlessSymId<'l> {
    self.with_extraction_mut(|xt| xt.fresh_id(name.clone(), vec![name]))
  }

  fn symbol_path_from_def_id(&self, def_id: DefId) -> Vec<String> {
    self
      .tcx
      .def_path_str(def_id)
      .split("::")
      .map(|s| s.into())
      .collect()
  }

  fn register_def(&mut self, def_id: DefId) -> StainlessSymId<'l> {
    let symbol_path = self.symbol_path_from_def_id(def_id);
    let name = symbol_path.last().unwrap().clone();

    // Prepend an underscore to numerical-only identifiers for compatibility
    // with stainless
    let (name, path) = if name.chars().all(char::is_numeric) {
      let new_name = format!("_{}", name);

      // Append new name to symbol path
      let path_len = symbol_path.len();
      let new_path: Vec<String> = symbol_path
        .into_iter()
        .take(path_len - 1)
        .chain(std::iter::once(new_name.clone()))
        .collect();

      (new_name, new_path)
    } else {
      (name, symbol_path)
    };

    self.with_extraction_mut(|xt| {
      let id = xt.fresh_id(name, path);
      assert!(xt.mapping.did_to_stid.insert(def_id, id).is_none());
      id
    })
  }

  #[inline]
  fn get_id_from_def(&self, def_id: DefId) -> Option<StainlessSymId<'l>> {
    self.with_extraction(|xt| xt.mapping.did_to_stid.get(&def_id).copied())
  }

  fn get_or_register_def(&mut self, def_id: DefId) -> StainlessSymId<'l> {
    self
      .get_id_from_def(def_id)
      .unwrap_or_else(|| self.register_def(def_id))
  }

  fn register_hir(&mut self, hir_id: HirId, name: String) -> StainlessSymId<'l> {
    let mut symbol_path = self.symbol_path_from_def_id(hir_id.owner.to_def_id());
    symbol_path.push(name.clone());

    self.with_extraction_mut(|xt| {
      let id = xt.fresh_id(name, symbol_path);
      assert!(xt.mapping.hid_to_stid.insert(hir_id, id).is_none());
      id
    })
  }

  /// ADTs and Functions

  fn add_adt(&mut self, id: StainlessSymId<'l>, adt: &'l st::ADTSort<'l>) {
    self.with_extraction_mut(|xt| {
      assert!(xt.adts.insert(id, adt).is_none());
    })
  }

  fn add_function_ref(&mut self, def_id: DefId) {
    self.with_extraction_mut(|xt| {
      assert!(xt.function_refs.insert(def_id));
    })
  }

  fn add_function(&mut self, id: StainlessSymId<'l>, fd: &'l st::FunDef<'l>) {
    self.with_extraction_mut(|xt| {
      assert!(xt.functions.insert(id, fd).is_none());
    })
  }

  /// Get a BodyExtractor for some item with a body (like a function)
  fn enter_body<T, F>(&mut self, hir_id: HirId, txtcx: TyExtractionCtxt<'l>, f: F) -> T
  where
    F: FnOnce(&mut BodyExtractor<'_, 'l, 'tcx>) -> T,
  {
    self.tcx.infer_ctxt().enter(|infcx| {
      // Note that upon its creation, BodyExtractor moves out our Extraction
      let mut bxtor = BodyExtractor::new(self, &infcx, hir_id, txtcx);
      let result = f(&mut bxtor);
      // We reclaim the Extraction after the BodyExtractor's work is done
      self.extraction = bxtor.base.extraction;
      result
    })
  }

  /// Error reporting helpers

  fn unsupported<S: Into<MultiSpan>, M: Into<String>>(&self, span: S, msg: M) {
    let msg = msg.into();
    self
      .tcx
      .sess
      .span_err(span, format!("Unsupported tree: {}", msg).as_str());
  }
}

/// BodyExtractor is used to extract, for example, function bodies
struct BodyExtractor<'a, 'l, 'tcx: 'l> {
  base: BaseExtractor<'l, 'tcx>,
  hcx: hair::cx::Cx<'a, 'tcx>,
  tables: &'a TypeckTables<'tcx>,
  body: &'tcx hir::Body<'tcx>,
  txtcx: TyExtractionCtxt<'l>,
  dcx: DefContext<'l>,
}

impl<'a, 'l, 'tcx> BodyExtractor<'a, 'l, 'tcx> {
  fn new(
    base: &mut BaseExtractor<'l, 'tcx>,
    infcx: &'a InferCtxt<'a, 'tcx>,
    hir_id: HirId,
    txtcx: TyExtractionCtxt<'l>,
  ) -> Self {
    let tcx = base.tcx;
    let extraction = base.extraction.take();
    let base = BaseExtractor::new(
      tcx,
      base.std_items.clone(),
      extraction.expect("Waiting for another BodyExtractor to finish"),
    );

    // Set up HAIR context
    let hcx = hair::cx::Cx::new(infcx, hir_id);

    // Set up typing tables and signature
    let def_id = tcx.hir().local_def_id(hir_id);
    assert!(tcx.has_typeck_tables(def_id));
    let tables = tcx.typeck_tables_of(def_id);

    // Fetch the body and the corresponding DefContext containing all bindings
    let body_id = tcx.hir().body_owned_by(hir_id);
    let body = tcx.hir().body(body_id);

    BodyExtractor {
      base,
      hcx,
      tables,
      body,
      txtcx,
      dcx: DefContext::new(),
    }
  }

  #[inline]
  fn tcx(&self) -> TyCtxt<'tcx> {
    self.hcx.tcx()
  }

  #[inline]
  fn factory(&self) -> &'l st::Factory {
    self.base.factory()
  }

  fn fetch_var(&self, hir_id: HirId) -> &'l st::Variable<'l> {
    let span: Span = self.tcx().hir().span(hir_id);
    self
      .dcx
      .get_var(hir_id)
      .unwrap_or_else(|| unexpected(span, "unregistered variable"))
  }
}

fn unexpected<S: Into<MultiSpan>, M: Into<String>>(span: S, msg: M) -> ! {
  span_bug!(span, "Unexpected tree: {:?}", msg.into())
}
