use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

// Opens a web or mailto link in the user's default app.
pub fn open_link(link: &str) -> Result<(), String> {
    // SAFETY: plain Win32 call with valid, NUL-terminated strings and no parent window.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(link),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW reports success with a value above 32.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!("Could not open {link}"))
    }
}
