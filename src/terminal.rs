#[cfg(unix)]
mod unix {
    pub fn use_fancy() -> bool {
        unsafe {
            libc::isatty(/* stdout */ 1) == 1
        }
    }

    pub fn get_cols() -> Option<usize> {
        unsafe {
            let mut winsize = std::mem::MaybeUninit::<libc::winsize>::uninit();
            if libc::ioctl(0, libc::TIOCGWINSZ, &mut winsize) < 0 {
                return None;
            }
            let winsize = winsize.assume_init();
            Some(winsize.ws_col as usize)
        }
    }
}

#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows {
    pub fn use_fancy() -> bool {
        unsafe {
            let handle =
                winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE);
            let mut out = 0;
            // Note: GetConsoleMode itself fails when not attached to a console.
            winapi::um::consoleapi::GetConsoleMode(handle, &mut out) != 0
        }
    }

    #[allow(clippy::uninit_assumed_init)]
    pub fn get_cols() -> Option<usize> {
        use kernel32::{GetConsoleScreenBufferInfo, GetStdHandle};
        let console = unsafe { GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE) };
        if console == winapi::um::handleapi::INVALID_HANDLE_VALUE {
            return None;
        }
        unsafe {
            let mut csbi = ::std::mem::MaybeUninit::uninit().assume_init();
            if GetConsoleScreenBufferInfo(console, &mut csbi) == 0 {
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
