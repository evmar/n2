//! Implements run_command on Windows using native Windows calls.
//! See run_command comments for why.

use crate::process::Termination;
use std::ffi::c_void;
use std::io::Read;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::os::windows::prelude::AsRawHandle;
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    System::{Console::*, Diagnostics::Debug::*, Pipes::CreatePipe, Threading::*},
};

/// Construct an error from GetLastError().
fn windows_error(func: &str) -> anyhow::Error {
    unsafe {
        let err = GetLastError();
        let mut buf: [u8; 1024] = [0; 1024];
        let len = FormatMessageA(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            err,
            0x0000_0400, // MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT)
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null(),
        );
        if len == 0 {
            panic!("FormatMessageA on error failed: {}", GetLastError());
        }
        let message = std::str::from_utf8(&buf[..len as usize])
            .unwrap()
            .trim_end();
        anyhow::anyhow!("{}: {}", func, message)
    }
}
/// Return an Err from the current function with GetLastError info in it.
macro_rules! win_bail {
    ($func:ident) => {
        return Err(windows_error(stringify!($func)));
    };
}

/// Wrapper for PROCESS_INFORMATION that cleans up on Drop.
struct ProcessInformation(PROCESS_INFORMATION);

impl ProcessInformation {
    fn new() -> Self {
        Self(unsafe { std::mem::zeroed() })
    }
    fn as_mut_ptr(&mut self) -> *mut PROCESS_INFORMATION {
        &mut self.0
    }
}

impl std::ops::Deref for ProcessInformation {
    type Target = PROCESS_INFORMATION;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ProcessInformation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ProcessInformation {
    fn drop(&mut self) {
        unsafe {
            if self.hProcess != 0 {
                CloseHandle(self.hProcess);
            }
            if self.hThread != 0 {
                CloseHandle(self.hThread);
            }
        }
    }
}

/// Wrapper for PROC_THREAD_ATTRIBUTE_LIST.  This is a type whose size we discover at runtime.
struct ProcThreadAttributeList(Box<[u8]>);
impl ProcThreadAttributeList {
    fn new(count: usize) -> anyhow::Result<Self> {
        unsafe {
            let mut size = 0;
            if InitializeProcThreadAttributeList(std::ptr::null_mut(), count as u32, 0, &mut size)
                == 0
            {
                if GetLastError() != ERROR_INSUFFICIENT_BUFFER {
                    win_bail!(InitializeProcThreadAttributeList);
                }
            }

            let mut buf = vec![0u8; size].into_boxed_slice();
            if InitializeProcThreadAttributeList(
                buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST,
                count as u32,
                0,
                &mut size,
            ) == 0
            {
                win_bail!(InitializeProcThreadAttributeList);
            }
            Ok(Self(buf))
        }
    }

    fn inherit_handles(&mut self, handles: &[HANDLE]) -> anyhow::Result<()> {
        unsafe {
            if UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr() as *const c_void,
                handles.len() * std::mem::size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                win_bail!(UpdateProcThreadAttribute);
            }
        }
        Ok(())
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
    }
}

pub fn run_command(cmdline: &str, mut output_cb: impl FnMut(&[u8])) -> anyhow::Result<Termination> {
    // Don't want to run `cmd /c` since that limits cmd line length to 8192 bytes.
    // std::process::Command can't take a string and pass it through to CreateProcess unchanged,
    // so call that ourselves.
    // https://github.com/rust-lang/rust/issues/38227

    let (pipe_read, pipe_write) = unsafe {
        let mut pipe_read: HANDLE = 0;
        let mut pipe_write: HANDLE = 0;
        let mut attrs = std::mem::zeroed::<SECURITY_ATTRIBUTES>();
        attrs.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
        attrs.bInheritHandle = TRUE;
        if CreatePipe(
            &mut pipe_read,
            &mut pipe_write,
            &mut attrs,
            /* use default buffer size */ 0,
        ) == 0
        {
            win_bail!(CreatePipe);
        }
        (
            OwnedHandle::from_raw_handle(pipe_read as *mut c_void),
            OwnedHandle::from_raw_handle(pipe_write as *mut c_void),
        )
    };

    let process_info = unsafe {
        // TODO: Set this to just 0 for console pool jobs.
        let process_flags = CREATE_NEW_PROCESS_GROUP | EXTENDED_STARTUPINFO_PRESENT;

        let mut startup_info = std::mem::zeroed::<STARTUPINFOEXA>();
        startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXA>() as u32;
        startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup_info.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        let raw_pipe_write = pipe_write.as_raw_handle() as isize;
        startup_info.StartupInfo.hStdOutput = raw_pipe_write;
        startup_info.StartupInfo.hStdError = raw_pipe_write;

        // Safely inherit in/out handles.
        // https://devblogs.microsoft.com/oldnewthing/20111216-00/?p=8873
        let mut attrs = ProcThreadAttributeList::new(1)?;
        attrs.inherit_handles(&[startup_info.StartupInfo.hStdInput, raw_pipe_write])?;
        startup_info.lpAttributeList = attrs.as_mut_ptr();

        let mut process_info = ProcessInformation::new();

        let mut cmdline_nul: Vec<u8> = String::from(cmdline).into_bytes();
        cmdline_nul.push(0);

        if CreateProcessA(
            std::ptr::null_mut(),
            cmdline_nul.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            /*inherit handles = */ TRUE,
            process_flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut startup_info.StartupInfo,
            process_info.as_mut_ptr(),
        ) == 0
        {
            win_bail!(CreateProcessA);
        }
        drop(pipe_write);

        process_info
    };

    let mut pipe = std::fs::File::from(pipe_read);
    let mut buf: [u8; 4 << 10] = [0; 4 << 10];
    loop {
        let n = pipe.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output_cb(&buf[0..n]);
    }

    let exit_code = unsafe {
        if WaitForSingleObject(process_info.hProcess, INFINITE) != 0 {
            win_bail!(WaitForSingleObject);
        }

        let mut exit_code: u32 = 0;
        if GetExitCodeProcess(process_info.hProcess, &mut exit_code) == 0 {
            win_bail!(GetExitCodeProcess);
        }

        exit_code
    };

    let termination = match exit_code {
        0 => Termination::Success,
        0xC000013A => Termination::Interrupted,
        _ => Termination::Failure,
    };

    Ok(termination)
}
