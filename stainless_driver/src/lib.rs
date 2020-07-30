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

pub fn run() -> Result<(), ()> {
  let mut callbacks = ExtractionCallbacks {};
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

struct ExtractionCallbacks {}

impl Callbacks for ExtractionCallbacks {
  fn config(&mut self, config: &mut interface::Config) {
    config.opts.debugging_opts.save_analysis = true;
  }

  fn after_expansion<'tcx>(
    &mut self,
    _compiler: &interface::Compiler,
    _queries: &'tcx Queries<'tcx>,
  ) -> Compilation {
    Compilation::Continue
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
        extraction::extract_and_output_crate(tcx, crate_name);
      });
    });

    Compilation::Stop
  }
}
