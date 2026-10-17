use std::path::{Path, PathBuf};

use webview2_com::take_pwstr;
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING};

// Blocks until the user picks a folder (Some) or cancels (None).
// Must run on a thread of its own: it owns a COM apartment.
pub fn pick_folder(start: &Path) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: initializes COM for this thread only; balanced by CoUninitialize below.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let picked = show(start);
    // SAFETY: every COM object created on this thread was dropped inside `show`.
    unsafe { CoUninitialize() };
    picked
}

fn show(start: &Path) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: standard IFileOpenDialog use on the thread that created it.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        if let Ok(folder) =
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None)
        {
            let _ = dialog.SetFolder(&folder);
        }
        match dialog.Show(None) {
            Err(err) if err.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(None),
            shown => shown?,
        }
        let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        Ok(Some(PathBuf::from(take_pwstr(path))))
    }
}
