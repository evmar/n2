#[cfg(unix)]
mod unix {
    pub fn use_fancy() -> bool {
        unsafe {
            libc::isatty(/* stdout */ 1) == 1
        }
    }

    pub fn get_cols() -> Option<usize> {
        unsafe {
            let mut winsize = std::mem::zeroed::<libc::winsize>();
            if libc::ioctl(0, libc::TIOCGWINSZ, &mut winsize) < 0 {
                return None;
            }
            if winsize.ws_col < 10 {
                // https://github.com/evmar/n2/issues/63: ignore too-narrow widths
                return None;
            }
            Some(winsize.ws_col as usize)
        }
    }
}

#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::{Foundation::*, System::Console::*};

    pub fn use_fancy() -> bool {
        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0;
            // Note: GetConsoleMode itself fails when not attached to a console.
            let ok = GetConsoleMode(handle, &mut mode) != 0;
            if ok {
                // Enable terminal processing so we can overwrite previous content.
                // Ignore errors.
                _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
            ok
        }
    }

    pub fn get_cols() -> Option<usize> {
        unsafe {
            let console = GetStdHandle(STD_OUTPUT_HANDLE);
            if console == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut csbi = ::std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>();
            if GetConsoleScreenBufferInfo(console, &mut csbi) == 0 {
                return None;
            }
            if csbi.dwSize.X < 10 {
                // https://github.com/evmar/n2/issues/63: ignore too-narrow widths
                return None;
            }
            Some(csbi.dwSize.X as usize)
        }
    }
}

#[cfg(windows)]
pub use windows::*;

#[cfg(target_arch = "wasm32")]
mod wasm {
    pub fn use_fancy() -> bool {
        false
    }

    pub fn get_cols() -> Option<usize> {
        None
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;
