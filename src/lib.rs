pub mod canon;
mod db;
mod densemap;
mod depfile;
mod eval;
mod graph;
mod hash;
mod load;
mod parse;
mod progress;
pub mod run;
mod scanner;
mod signal;
mod smallmap;
mod task;
mod terminal;
mod trace;
mod work;

#[cfg(unix)]
#[macro_use]
extern crate lazy_static;

#[cfg(not(any(windows, target_arch = "wasm32")))]
use jemallocator::Jemalloc;

#[cfg(not(any(windows, target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
