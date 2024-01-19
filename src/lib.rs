pub mod canon;
mod db;
mod densemap;
mod depfile;
mod eval;
mod graph;
mod hash;
pub mod load;
pub mod parse;
mod process;
#[cfg(unix)]
mod process_posix;
#[cfg(windows)]
mod process_win;
mod progress;
pub mod run;
pub mod scanner;
mod signal;
mod smallmap;
mod task;
mod terminal;
mod trace;
mod work;

#[cfg(not(any(windows, target_arch = "wasm32")))]
use jemallocator::Jemalloc;

#[cfg(not(any(windows, target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
