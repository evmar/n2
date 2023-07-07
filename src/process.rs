#[cfg(unix)]
pub use crate::process_posix::run_command;
#[cfg(windows)]
pub use crate::process_win::run_command;

#[derive(PartialEq)]
pub enum Termination {
    Success,
    Interrupted,
    Failure,
}
