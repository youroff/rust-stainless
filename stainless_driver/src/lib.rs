#![feature(rustc_private)]
#![feature(box_patterns)]
#![allow(clippy::unused_unit, clippy::let_and_return)]

pub mod extraction;

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

use rustc_driver::{run_compiler, Callbacks, Compilation};
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_interface::{interface, Queries};
use rustc_session::config::ErrorOutputType;
use rustc_session::early_error;

use stainless_data::ast as st;

pub fn run<E: FnOnce(st::Symbols<'_>) + Send>(on_extraction: E) -> Result<(), ()> {
  let mut callbacks = ExtractionCallbacks::new(on_extraction);
  let file_loader = None;

  let args = std::env::args_os()
    .enumerate()
    .map(|(i, arg)| {
      arg.into_string().unwrap_or_else(|arg| {
        early_error(
          ErrorOutputType::default(),
          &format!("Argument {} is not valid Unicode: {:?}", i, arg),
        )
      })
    })
    .collect::<Vec<_>>();
  rustc_driver::install_ice_hook();
  rustc_driver::catch_fatal_errors(|| run_compiler(&args, &mut callbacks, file_loader, None))
    .map(|_| ())
    .map_err(|_| ())
}

struct ExtractionCallbacks<E>
where
  E: FnOnce(st::Symbols<'_>) + Send,
{
  on_extraction: Option<E>,
}

impl<E: FnOnce(st::Symbols<'_>) + Send> ExtractionCallbacks<E> {
  fn new(on_extraction: E) -> Self {
    Self {
      on_extraction: Some(on_extraction),
    }
  }
}

impl<E: FnOnce(st::Symbols<'_>) + Send> Callbacks for ExtractionCallbacks<E> {
  fn config(&mut self, config: &mut interface::Config) {
    config.opts.debugging_opts.save_analysis = true;
  }

  fn after_analysis<'tcx>(
    &mut self,
    _compiler: &interface::Compiler,
    queries: &'tcx Queries<'tcx>,
  ) -> Compilation {
    let crate_name = queries.crate_name().unwrap().peek().clone();

    queries.global_ctxt().unwrap().peek_mut().enter(|tcx| {
      tcx.dep_graph.with_ignore(|| {
        eprintln!("=== Analysing crate '{}' ===\n", crate_name);
        tcx.analysis(LOCAL_CRATE).unwrap();

        let factory = st::Factory::new();
        let symbols = extraction::extract_crate(tcx, &factory, crate_name);
        (self.on_extraction.take().expect("Already ran extraction"))(symbols);
      });
    });

    Compilation::Stop
  }
}

pub fn output_program<P: AsRef<std::path::Path>>(path: P, symbols: st::Symbols) -> () {
  use stainless_data::ser::{BufferSerializer, Serializable};
  let mut ser = BufferSerializer::new();
  symbols
    .serialize(&mut ser)
    .expect("Unable to serialize stainless program");
  std::fs::write(path, ser.as_slice()).expect("Unable to write serialized stainless program");
}
