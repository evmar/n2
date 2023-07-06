//! Runs build tasks, potentially in parallel.
//! Unaware of the build graph, pools, etc.; just command execution.
//!
//! TODO: consider rewriting to use poll() etc. instead of threads.
//! The threads might be relatively cheap(?) because they just block on
//! the subprocesses though?

use crate::{
    depfile,
    graph::{BuildId, RspFile},
    scanner::Scanner,
};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

#[cfg(unix)]
use std::io::Write;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[cfg(windows)]
extern crate winapi;

pub struct FinishedTask {
    /// A (faked) "thread id", used to put different finished builds in different
    /// tracks in a performance trace.
    pub tid: usize,
    pub buildid: BuildId,
    pub span: (Instant, Instant),
    pub result: TaskResult,
}

#[derive(PartialEq)]
pub enum Termination {
    Success,
    Interrupted,
    Failure,
}

/// The result of executing a build step.
pub struct TaskResult {
    pub termination: Termination,
    /// Console output.
    pub output: Vec<u8>,
    pub discovered_deps: Option<Vec<String>>,
}

/// Reads dependencies from a .d file path.
fn read_depfile(path: &Path) -> anyhow::Result<Vec<String>> {
    let mut bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => bail!("read {}: {}", path.display(), e),
    };
    let mut scanner = Scanner::new(&mut bytes);
    let parsed_deps = depfile::parse(&mut scanner)
        .map_err(|err| anyhow!(scanner.format_parse_error(path, err)))?;
    // TODO verify deps refers to correct output
    let deps: Vec<String> = parsed_deps
        .deps
        .iter()
        .map(|&dep| dep.to_string())
        .collect();
    Ok(deps)
}

fn write_rspfile(rspfile: &RspFile) -> anyhow::Result<()> {
    if let Some(parent) = rspfile.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&rspfile.path, &rspfile.content)?;
    Ok(())
}

/// Executes a build task as a subprocess.
/// Returns an Err() if we failed outside of the process itself.
fn run_task(
    cmdline: &str,
    depfile: Option<&Path>,
    rspfile: Option<&RspFile>,
) -> anyhow::Result<TaskResult> {
    if let Some(rspfile) = rspfile {
        write_rspfile(rspfile)?;
    }
    let mut result = run_command(cmdline)?;
    if result.termination == Termination::Success {
        if let Some(depfile) = depfile {
            result.discovered_deps = Some(read_depfile(depfile)?);
        }
    }
    Ok(result)
}

#[cfg(unix)]
fn check_posix(func: &str, ret: libc::c_int) -> anyhow::Result<()> {
    if ret < 0 {
        let err_str = unsafe { std::ffi::CStr::from_ptr(libc::strerror(ret)) };
        bail!("{}: {}", func, err_str.to_str().unwrap());
    }
    Ok(())
}

#[cfg(unix)]
struct PosixSpawnFileActions(libc::posix_spawn_file_actions_t);

#[cfg(unix)]
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

#[cfg(unix)]
impl Drop for PosixSpawnFileActions {
    fn drop(&mut self) {
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

#[cfg(unix)]
fn run_command(cmdline: &str) -> anyhow::Result<TaskResult> {
    use std::io::Read;

    // Spawn the subprocess using posix_spawn with output redirected to the pipe.
    // We don't use Rust's process spawning because of issue #14 and because
    // we want to feed both stdout and stderr into the same pipe, which cannot
    // be done with the existing std::process API.
    let (pid, mut pipe) = unsafe {
        use libc::*;
        use std::os::fd::FromRawFd;

        let mut pipe: [c_int; 2] = std::mem::zeroed();
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

    Ok(TaskResult {
        termination,
        output,
        discovered_deps: None,
    })
}

#[cfg(windows)]
fn zeroed_startupinfo() -> winapi::um::processthreadsapi::STARTUPINFOA {
    winapi::um::processthreadsapi::STARTUPINFOA {
        cb: 0,
        lpReserved: std::ptr::null_mut(),
        lpDesktop: std::ptr::null_mut(),
        lpTitle: std::ptr::null_mut(),
        dwX: 0,
        dwY: 0,
        dwXSize: 0,
        dwYSize: 0,
        dwXCountChars: 0,
        dwYCountChars: 0,
        dwFillAttribute: 0,
        dwFlags: 0,
        wShowWindow: 0,
        cbReserved2: 0,
        lpReserved2: std::ptr::null_mut(),
        hStdInput: winapi::um::handleapi::INVALID_HANDLE_VALUE,
        hStdOutput: winapi::um::handleapi::INVALID_HANDLE_VALUE,
        hStdError: winapi::um::handleapi::INVALID_HANDLE_VALUE,
    }
}

#[cfg(windows)]
fn zeroed_process_information() -> winapi::um::processthreadsapi::PROCESS_INFORMATION {
    winapi::um::processthreadsapi::PROCESS_INFORMATION {
        hProcess: std::ptr::null_mut(),
        hThread: std::ptr::null_mut(),
        dwProcessId: 0,
        dwThreadId: 0,
    }
}

#[cfg(windows)]
fn run_command(cmdline: &str) -> anyhow::Result<TaskResult> {
    // Don't want to run `cmd /c` since that limits cmd line length to 8192 bytes.
    // std::process::Command can't take a string and pass it through to CreateProcess unchanged,
    // so call that ourselves.

    // TODO: Set this to just 0 for console pool jobs.
    let process_flags = winapi::um::winbase::CREATE_NEW_PROCESS_GROUP;

    let mut startup_info = zeroed_startupinfo();
    startup_info.cb = std::mem::size_of::<winapi::um::processthreadsapi::STARTUPINFOA>() as u32;
    startup_info.dwFlags = winapi::um::winbase::STARTF_USESTDHANDLES;

    let mut process_info = zeroed_process_information();

    let mut mut_cmdline = cmdline.to_string() + "\0";

    let create_process_success = unsafe {
        winapi::um::processthreadsapi::CreateProcessA(
            std::ptr::null_mut(),
            mut_cmdline.as_mut_ptr() as *mut i8,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            /*inherit handles = */ winapi::shared::ntdef::TRUE.into(),
            process_flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut startup_info,
            &mut process_info,
        )
    };
    if create_process_success == 0 {
        // TODO: better error?
        let error = unsafe { winapi::um::errhandlingapi::GetLastError() };
        bail!("CreateProcessA failed: {}", error);
    }

    unsafe {
        winapi::um::handleapi::CloseHandle(process_info.hThread);
    }

    unsafe {
        winapi::um::synchapi::WaitForSingleObject(
            process_info.hProcess,
            winapi::um::winbase::INFINITE,
        );
    }

    let mut exit_code: u32 = 0;
    unsafe {
        winapi::um::processthreadsapi::GetExitCodeProcess(process_info.hProcess, &mut exit_code);
    }

    unsafe {
        winapi::um::handleapi::CloseHandle(process_info.hProcess);
    }

    let output = Vec::new();
    // TODO: Set up pipes so that we can print the process's output.
    //output.append(&mut cmd.stdout);
    //output.append(&mut cmd.stderr);
    let termination = match exit_code {
        0 => Termination::Success,
        0xC000013A => Termination::Interrupted,
        _ => Termination::Failure,
    };

    Ok(TaskResult {
        termination,
        output,
        discovered_deps: None,
    })
}

#[cfg(target_arch = "wasm32")]
fn run_command(cmdline: &str) -> anyhow::Result<TaskResult> {
    bail!("wasm cannot run commands");
}

/// Tracks faked "thread ids" -- integers assigned to build tasks to track
/// parallelism in perf trace output.
#[derive(Default)]
struct ThreadIds {
    /// An entry is true when claimed, false or nonexistent otherwise.
    slots: Vec<bool>,
}
impl ThreadIds {
    fn claim(&mut self) -> usize {
        match self.slots.iter().position(|&used| !used) {
            Some(idx) => {
                self.slots[idx] = true;
                idx
            }
            None => {
                let idx = self.slots.len();
                self.slots.push(true);
                idx
            }
        }
    }

    fn release(&mut self, slot: usize) {
        self.slots[slot] = false;
    }
}

pub struct Runner {
    finished_send: mpsc::Sender<FinishedTask>,
    finished_recv: mpsc::Receiver<FinishedTask>,
    pub running: usize,
    tids: ThreadIds,
    parallelism: usize,
}

impl Runner {
    pub fn new(parallelism: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        Runner {
            finished_send: tx,
            finished_recv: rx,
            running: 0,
            tids: ThreadIds::default(),
            parallelism,
        }
    }

    pub fn can_start_more(&self) -> bool {
        self.running < self.parallelism
    }

    pub fn is_running(&self) -> bool {
        self.running > 0
    }

    pub fn start(
        &mut self,
        id: BuildId,
        cmdline: String,
        depfile: Option<PathBuf>,
        rspfile: Option<RspFile>,
    ) {
        let tid = self.tids.claim();
        let tx = self.finished_send.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result =
                run_task(&cmdline, depfile.as_deref(), rspfile.as_ref()).unwrap_or_else(|err| {
                    TaskResult {
                        termination: Termination::Failure,
                        output: err.to_string().into_bytes(),
                        discovered_deps: None,
                    }
                });
            let finish = Instant::now();

            let task = FinishedTask {
                tid,
                buildid: id,
                span: (start, finish),
                result,
            };
            // The send will only fail if the receiver disappeared, e.g. due to shutting down.
            let _ = tx.send(task);
        });
        self.running += 1;
    }

    /// Wait for a build to complete.  May block for a long time.
    pub fn wait(&mut self) -> FinishedTask {
        let task = self.finished_recv.recv().unwrap();
        self.tids.release(task.tid);
        self.running -= 1;
        task
    }
}
