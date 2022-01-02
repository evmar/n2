pub mod canon;
mod db;
mod depfile;
mod eval;
mod graph;
pub mod load;
mod parse;
pub mod progress;
mod run;
mod scanner;
pub mod trace;
pub mod work;

use jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
