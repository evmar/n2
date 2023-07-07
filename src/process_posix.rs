//! Implements run_command on posix using posix_spawn.
//! See run_command comments for why.

use crate::process::Termination;
use std::io::Read;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::process::ExitStatusExt;

fn check_posix(func: &str, ret: libc::c_int) -> anyhow::Result<()> {
    if ret < 0 {
        let err_str = unsafe { std::ffi::CStr::from_ptr(libc::strerror(ret)) };
        anyhow::bail!("{}: {}", func, err_str.to_str().unwrap());
    }
    Ok(())
}

/// Wraps libc::posix_spawn_file_actions_t, in particular to implement Drop.
struct PosixSpawnFileActions(libc::posix_spawn_file_actions_t);

impl PosixSpawnFileActions {
    fn new() -> anyhow::Result<Self> {
        unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            check_posix(
                "posix_spawn_file_actions_init",
                libc::posix_spawn_file_actions_init(&mut actions),
            )?;
            Ok(Self(actions))
        }
    }

    fn as_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        &mut self.0
    }

    fn adddup2(&mut self, fd: i32, newfd: i32) -> anyhow::Result<()> {
        unsafe {
            check_posix(
                "posix_spawn_file_actions_adddup2",
                libc::posix_spawn_file_actions_adddup2(&mut self.0, fd, newfd),
            )
        }
    }

    fn addclose(&mut self, fd: i32) -> anyhow::Result<()> {
        unsafe {
            check_posix(
                "posix_spawn_file_actions_addclose",
                libc::posix_spawn_file_actions_addclose(&mut self.0, fd),
            )
        }
    }
}

impl Drop for PosixSpawnFileActions {
    fn drop(&mut self) {
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

pub fn run_command(cmdline: &str) -> anyhow::Result<(Termination, Vec<u8>)> {
    // Spawn the subprocess using posix_spawn with output redirected to the pipe.
    // We don't use Rust's process spawning because of issue #14 and because
    // we want to feed both stdout and stderr into the same pipe, which cannot
    // be done with the existing std::process API.
    let (pid, mut pipe) = unsafe {
        let mut pipe: [libc::c_int; 2] = std::mem::zeroed();
        check_posix("pipe", libc::pipe(&mut pipe as *mut i32))?;

        let mut actions = PosixSpawnFileActions::new()?;
        // stdout/stderr => pipe
        actions.adddup2(pipe[1], 1)?;
        actions.adddup2(pipe[1], 2)?;
        // close pipe in child
        actions.addclose(pipe[0])?;
        actions.addclose(pipe[1])?;

        let mut pid: libc::pid_t = 0;
        let path = "/bin/sh\0".as_ptr() as *const i8;
        let cmdline_nul = std::ffi::CString::new(cmdline).unwrap();
        let argv: [*const i8; 4] = [
            path,
            "-c\0".as_ptr() as *const i8,
            cmdline_nul.as_ptr() as *const i8,
            std::ptr::null(),
        ];

        check_posix(
            "posix_spawn",
            libc::posix_spawn(
                &mut pid,
                path,
                actions.as_ptr(),
                std::ptr::null(),
                std::mem::transmute(&argv),
                std::ptr::null(),
            ),
        )?;

        check_posix("close", libc::close(pipe[1]))?;

        (pid, std::fs::File::from_raw_fd(pipe[0]))
    };

    let mut output = Vec::new();
    pipe.read_to_end(&mut output)?;

    let status = unsafe {
        let mut status: i32 = 0;
        check_posix("waitpid", libc::waitpid(pid, &mut status, 0))?;
        std::process::ExitStatus::from_raw(status)
    };

    let mut termination = Termination::Success;
    if !status.success() {
        termination = Termination::Failure;
        if let Some(sig) = status.signal() {
            match sig {
                libc::SIGINT => {
                    write!(output, "interrupted").unwrap();
                    termination = Termination::Interrupted;
                }
                _ => write!(output, "signal {}", sig).unwrap(),
            }
        }
    }

    Ok((termination, output))
}
