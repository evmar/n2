//! Runs build tasks, potentially in parallel.
//! Unaware of the build graph, pools, etc.; just command execution.
//!
//! TODO: consider rewriting to use poll() etc. instead of threads.
//! The threads might be relatively cheap(?) because they just block on
//! the subprocesses though?

use crate::depfile;
use crate::graph::{BuildId, RspFile};
use crate::scanner::Scanner;
use anyhow::{anyhow, bail};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::io::Write;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[cfg(unix)]
use std::sync::Mutex;

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

/// The result of executing a build step.
pub struct TaskResult {
    pub success: bool,
    /// Console output.
    pub output: Vec<u8>,
    pub discovered_deps: Option<Vec<String>>,
}

/// Reads dependencies from a .d file path.
fn read_depfile(path: &str) -> anyhow::Result<Vec<String>> {
    let mut bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => bail!("read {}: {}", path, e),
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
    depfile: Option<&str>,
    rspfile: Option<&RspFile>,
) -> anyhow::Result<TaskResult> {
    if let Some(rspfile) = rspfile {
        write_rspfile(rspfile)?;
    }
    let mut result = run_command(cmdline)?;
    if result.success {
        if let Some(depfile) = depfile {
            result.discovered_deps = Some(read_depfile(depfile)?);
        }
    }
    Ok(result)
}

#[cfg(unix)]
lazy_static! {
    static ref TASK_MUTEX: Mutex<i32> = Mutex::new(0);
}

#[cfg(unix)]
fn run_command(cmdline: &str) -> anyhow::Result<TaskResult> {
    // Command::spawn() can leak FSs when run concurrently, see #14.
    let just_one = TASK_MUTEX.lock().unwrap();
    let p = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmdline)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    drop(just_one);

    let mut cmd = p.wait_with_output()?;
    let mut output = Vec::new();
    output.append(&mut cmd.stdout);
    output.append(&mut cmd.stderr);
    let success = cmd.status.success();

    if !success {
        if let Some(sig) = cmd.status.signal() {
            match sig {
                libc::SIGINT => write!(output, "interrupted").unwrap(),
                _ => write!(output, "signal {}", sig).unwrap(),
            }
        }
    }

    Ok(TaskResult {
        success,
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

    let mut output = Vec::new();
    // TODO: Set up pipes so that we can print the process's output.
    //output.append(&mut cmd.stdout);
    //output.append(&mut cmd.stderr);
    let success = exit_code == 0;

    Ok(TaskResult {
        success,
        output,
        discovered_deps: None,
    })
}

/// Tracks faked "thread ids" -- integers assigned to build tasks to track
/// paralllelism in perf trace output.
struct ThreadIds {
    /// An entry is true when claimed, false or nonexistent otherwise.
    slots: Vec<bool>,
}
impl ThreadIds {
    fn new() -> Self {
        ThreadIds { slots: Vec::new() }
    }

    fn claim(&mut self) -> usize {
        match self.slots.iter().position(|&used| !used) {
            Some(idx) => {
                self.slots[idx] = true;
                idx
            }
            None => {
                let idx = self.slots.len();
                self.slots.push(false);
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
            tids: ThreadIds::new(),
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
        depfile: Option<String>,
        rspfile: Option<RspFile>,
    ) {
        let tid = self.tids.claim();
        let tx = self.finished_send.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result =
                run_task(&cmdline, depfile.as_deref(), rspfile.as_ref()).unwrap_or_else(|err| {
                    TaskResult {
                        success: false,
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

    /// Wait for a build to complete, with a timeout.
    /// If the timeout elapses return None.
    pub fn wait(&mut self, dur: Duration) -> Option<FinishedTask> {
        let task = match self.finished_recv.recv_timeout(dur) {
            Err(mpsc::RecvTimeoutError::Timeout) => return None,
            // The unwrap() checks the recv() call, to panic on mpsc errors.
            r => r.unwrap(),
        };
        self.tids.release(task.tid);
        self.running -= 1;
        Some(task)
    }
}
