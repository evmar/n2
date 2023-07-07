//! Exposes process::run_command, a wrapper around platform-native process execution.

#[cfg(unix)]
pub use crate::process_posix::run_command;
#[cfg(windows)]
pub use crate::process_win::run_command;

#[cfg(target_arch = "wasm32")]
fn run_command(cmdline: &str) -> anyhow::Result<(Termination, Vec<u8>)> {
    anyhow::bail!("wasm cannot run commands");
}

#[derive(PartialEq)]
pub enum Termination {
    Success,
    Interrupted,
    Failure,
}
