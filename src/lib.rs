pub mod canon;
mod db;
mod densemap;
mod depfile;
mod eval;
mod graph;
pub mod load;
pub mod parse;
pub mod progress;
mod scanner;
mod signal;
mod task;
pub mod trace;
pub mod work;

use jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
