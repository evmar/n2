pub mod canon;
mod db;
mod depfile;
mod eval;
mod graph;
pub mod load;
pub mod parse;
pub mod progress;
mod run;
pub mod scanner;
mod signal;
pub mod trace;
pub mod work;

use jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
