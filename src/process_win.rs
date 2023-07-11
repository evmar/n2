//! Implements run_command on Windows using native Windows calls.
//! See run_command comments for why.

use windows_sys::Win32::{
    Foundation::*,
    System::{Console::*, Threading::*},
};

use crate::process::Termination;

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

#[allow(non_snake_case)]
fn GetLastError() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

pub fn run_command(cmdline: &str, _output_cb: impl FnMut(&[u8])) -> anyhow::Result<Termination> {
    // Don't want to run `cmd /c` since that limits cmd line length to 8192 bytes.
    // std::process::Command can't take a string and pass it through to CreateProcess unchanged,
    // so call that ourselves.

    let exit_code = unsafe {
        // TODO: Set this to just 0 for console pool jobs.
        let process_flags = CREATE_NEW_PROCESS_GROUP;

        let mut startup_info = std::mem::zeroed::<STARTUPINFOA>();
        startup_info.cb = std::mem::size_of::<STARTUPINFOA>() as u32;
        startup_info.dwFlags = STARTF_USESTDHANDLES;
        startup_info.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        startup_info.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
        startup_info.hStdError = startup_info.hStdOutput;

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
            &mut startup_info,
            process_info.as_mut_ptr(),
        ) == 0
        {
            anyhow::bail!("{}: {}", "CreateProcessA", GetLastError());
        }

        if WaitForSingleObject(process_info.hProcess, INFINITE) != 0 {
            anyhow::bail!("{}: {}", "WaitForSingleObject", GetLastError());
        }

        let mut exit_code: u32 = 0;
        if GetExitCodeProcess(process_info.hProcess, &mut exit_code) == 0 {
            anyhow::bail!("{}: {}", "GetExitCodeProcess", GetLastError());
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
