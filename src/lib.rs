pub mod canon;
mod db;
mod depfile;
mod eval;
pub mod fs;
pub mod graph;
pub mod load;
pub mod parse;
pub mod progress;
pub mod scanner;
mod signal;
mod task;
pub mod trace;
pub mod work;

use jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
