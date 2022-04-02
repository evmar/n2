pub mod canon;
mod db;
mod densemap;
mod depfile;
mod eval;
mod graph;
mod load;
mod parse;
mod progress;
pub mod run;
mod scanner;
mod signal;
mod task;
mod trace;
mod work;

#[cfg(unix)]
#[macro_use]
extern crate lazy_static;

#[cfg(not(windows))]
use jemallocator::Jemalloc;

#[cfg(not(windows))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
