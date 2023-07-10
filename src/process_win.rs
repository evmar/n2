//! Implements run_command on Windows using native Windows calls.
//! See run_command comments for why.

extern crate winapi;

use crate::process::Termination;

#[allow(non_snake_case)]
fn GetLastError() -> u32 {
    unsafe { winapi::um::errhandlingapi::GetLastError() }
}

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

fn zeroed_process_information() -> winapi::um::processthreadsapi::PROCESS_INFORMATION {
    winapi::um::processthreadsapi::PROCESS_INFORMATION {
        hProcess: std::ptr::null_mut(),
        hThread: std::ptr::null_mut(),
        dwProcessId: 0,
        dwThreadId: 0,
    }
}

pub fn run_command(cmdline: &str, _output_cb: impl FnMut(&[u8])) -> anyhow::Result<Termination> {
    // Don't want to run `cmd /c` since that limits cmd line length to 8192 bytes.
    // std::process::Command can't take a string and pass it through to CreateProcess unchanged,
    // so call that ourselves.

    let exit_code = unsafe {
        // TODO: Set this to just 0 for console pool jobs.
        let process_flags = winapi::um::winbase::CREATE_NEW_PROCESS_GROUP;

        let mut startup_info = zeroed_startupinfo();
        startup_info.cb = std::mem::size_of::<winapi::um::processthreadsapi::STARTUPINFOA>() as u32;
        startup_info.dwFlags = winapi::um::winbase::STARTF_USESTDHANDLES;
        startup_info.hStdInput =
            winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_INPUT_HANDLE);
        startup_info.hStdOutput =
            winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE);
        startup_info.hStdError = startup_info.hStdOutput;

        let mut process_info = zeroed_process_information();

        let mut mut_cmdline = cmdline.to_string() + "\0";

        if winapi::um::processthreadsapi::CreateProcessA(
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
        ) == 0
        {
            anyhow::bail!("{}: {}", "CreateProcessA", GetLastError());
        }

        if winapi::um::handleapi::CloseHandle(process_info.hThread) == 0 {
            anyhow::bail!("{}: {}", "CloseHandle", GetLastError());
        }

        if winapi::um::synchapi::WaitForSingleObject(
            process_info.hProcess,
            winapi::um::winbase::INFINITE,
        ) != 0
        {
            anyhow::bail!("{}: {}", "WaitForSingleObject", GetLastError());
        }

        let mut exit_code: u32 = 0;
        if winapi::um::processthreadsapi::GetExitCodeProcess(process_info.hProcess, &mut exit_code)
            == 0
        {
            anyhow::bail!("{}: {}", "GetExitCodeProcess", GetLastError());
        }

        if winapi::um::handleapi::CloseHandle(process_info.hProcess) == 0 {
            anyhow::bail!("{}: {}", "CloseHandle", GetLastError());
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
