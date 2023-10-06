//! Implements run_command on posix using posix_spawn.
//! See run_command comments for why.

use crate::process::Termination;
use std::io::{Error, Read};
use std::os::fd::FromRawFd;
use std::os::unix::process::ExitStatusExt;

// https://github.com/rust-lang/libc/issues/2520
// libc crate doesn't expose the 'environ' pointer.
extern "C" {
    static environ: *const *mut libc::c_char;
}

fn check_posix_spawn(func: &str, ret: libc::c_int) -> anyhow::Result<()> {
    if ret != 0 {
        let err_str = unsafe { std::ffi::CStr::from_ptr(libc::strerror(ret)) };
        anyhow::bail!("{}: {}", func, err_str.to_str().unwrap());
    }
    Ok(())
}

fn check_ret_errno(func: &str, ret: libc::c_int) -> anyhow::Result<()> {
    if ret < 0 {
        let errno = Error::last_os_error().raw_os_error().unwrap();
        let err_str = unsafe { std::ffi::CStr::from_ptr(libc::strerror(errno)) };
        anyhow::bail!("{}: {}", func, err_str.to_str().unwrap());
    }
    Ok(())
}

/// Wraps libc::posix_spawnattr_t, in particular to implement Drop.
struct PosixSpawnAttr(libc::posix_spawnattr_t);

impl PosixSpawnAttr {
    fn new() -> anyhow::Result<Self> {
        unsafe {
            let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
            check_posix_spawn(
                "posix_spawnattr_init",
                libc::posix_spawnattr_init(&mut attr),
            )?;
            Ok(Self(attr))
        }
    }

    fn as_ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        &mut self.0
    }

    fn setflags(&mut self, flags: libc::c_short) -> anyhow::Result<()> {
        unsafe {
            check_posix_spawn(
                "posix_spawnattr_setflags",
                libc::posix_spawnattr_setflags(self.as_ptr(), flags),
            )
        }
    }
}

impl Drop for PosixSpawnAttr {
    fn drop(&mut self) {
        unsafe {
            libc::posix_spawnattr_destroy(self.as_ptr());
        }
    }
}

/// Wraps libc::posix_spawn_file_actions_t, in particular to implement Drop.
struct PosixSpawnFileActions(libc::posix_spawn_file_actions_t);

impl PosixSpawnFileActions {
    fn new() -> anyhow::Result<Self> {
        unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            check_posix_spawn(
                "posix_spawn_file_actions_init",
                libc::posix_spawn_file_actions_init(&mut actions),
            )?;
            Ok(Self(actions))
        }
    }

    fn as_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        &mut self.0
    }

    fn addopen(
        &mut self,
        fd: i32,
        path: &std::ffi::CStr,
        oflag: i32,
        mode: libc::mode_t,
    ) -> anyhow::Result<()> {
        unsafe {
            check_posix_spawn(
                "posix_spawn_file_actions_addopen",
                libc::posix_spawn_file_actions_addopen(
                    self.as_ptr(),
                    fd,
                    path.as_ptr(),
                    oflag,
                    mode,
                ),
            )
        }
    }

    fn adddup2(&mut self, fd: i32, newfd: i32) -> anyhow::Result<()> {
        unsafe {
            check_posix_spawn(
                "posix_spawn_file_actions_adddup2",
                libc::posix_spawn_file_actions_adddup2(self.as_ptr(), fd, newfd),
            )
        }
    }

    fn addclose(&mut self, fd: i32) -> anyhow::Result<()> {
        unsafe {
            check_posix_spawn(
                "posix_spawn_file_actions_addclose",
                libc::posix_spawn_file_actions_addclose(self.as_ptr(), fd),
            )
        }
    }
}

impl Drop for PosixSpawnFileActions {
    fn drop(&mut self) {
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

/// Create an anonymous pipe as in libc::pipe(), but using pipe2() when available
/// to set CLOEXEC flag.
fn pipe2() -> anyhow::Result<[libc::c_int; 2]> {
    // Compare to: https://doc.rust-lang.org/src/std/sys/unix/pipe.rs.html
    unsafe {
        let mut pipe: [libc::c_int; 2] = std::mem::zeroed();

        // Mac: specially handled below with POSIX_SPAWN_CLOEXEC_DEFAULT
        #[cfg(target_os = "macos")]
        check_ret_errno("pipe", libc::pipe(pipe.as_mut_ptr()))?;

        // Assume all non-Mac have pipe2; we can refine this on user feedback.
        #[cfg(all(unix, not(target_os = "macos")))]
        check_ret_errno("pipe", libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC))?;

        Ok(pipe)
    }
}

pub fn run_command(cmdline: &str, mut output_cb: impl FnMut(&[u8])) -> anyhow::Result<Termination> {
    // Spawn the subprocess using posix_spawn with output redirected to the pipe.
    // We don't use Rust's process spawning because of issue #14 and because
    // we want to feed both stdout and stderr into the same pipe, which cannot
    // be done with the existing std::process API.
    let (pid, mut pipe) = unsafe {
        let pipe = pipe2()?;

        let mut attr = PosixSpawnAttr::new()?;

        // Apple-specific extension: close any open fds.
        #[cfg(target_os = "macos")]
        attr.setflags(libc::POSIX_SPAWN_CLOEXEC_DEFAULT as _)?;

        let mut actions = PosixSpawnFileActions::new()?;
        // open /dev/null over stdin
        actions.addopen(
            0,
            std::ffi::CStr::from_bytes_with_nul_unchecked(b"/dev/null\0"),
            libc::O_RDONLY,
            0,
        )?;
        // stdout/stderr => pipe
        actions.adddup2(pipe[1], 1)?;
        actions.adddup2(pipe[1], 2)?;
        // close pipe in child
        actions.addclose(pipe[0])?;
        actions.addclose(pipe[1])?;

        let mut pid: libc::pid_t = 0;
        let path = std::ffi::CStr::from_bytes_with_nul_unchecked(b"/bin/sh\0");
        let cmdline_nul = std::ffi::CString::new(cmdline).unwrap();
        let argv: [*const libc::c_char; 4] = [
            path.as_ptr(),
            b"-c\0".as_ptr() as *const _,
            cmdline_nul.as_ptr(),
            std::ptr::null(),
        ];

        check_posix_spawn(
            "posix_spawn",
            libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                actions.as_ptr(),
                attr.as_ptr(),
                // posix_spawn wants mutable argv:
                // https://stackoverflow.com/questions/50596439/can-string-literals-be-passed-in-posix-spawns-argv
                argv.as_ptr() as *const *mut _,
                environ,
            ),
        )?;

        check_ret_errno("close", libc::close(pipe[1]))?;

        (pid, std::fs::File::from_raw_fd(pipe[0]))
    };

    let mut buf: [u8; 4 << 10] = [0; 4 << 10];
    loop {
        let n = pipe.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output_cb(&buf[0..n]);
    }
    drop(pipe);

    let status = unsafe {
        let mut status: i32 = 0;
        check_ret_errno("waitpid", libc::waitpid(pid, &mut status, 0))?;
        std::process::ExitStatus::from_raw(status)
    };

    let termination = if status.success() {
        Termination::Success
    } else if let Some(sig) = status.signal() {
        match sig {
            libc::SIGINT => {
                output_cb("interrupted".as_bytes());
                Termination::Interrupted
            }
            _ => {
                output_cb(format!("signal {}", sig).as_bytes());
                Termination::Failure
            }
        }
    } else {
        Termination::Failure
    };

    Ok(termination)
}
