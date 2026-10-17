use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::rc::Rc;

use thiserror::Error;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_STATE_DENY, ICoreWebView2, ICoreWebView2_2, ICoreWebView2_4,
    ICoreWebView2Controller, ICoreWebView2CookieList, ICoreWebView2Environment,
    ICoreWebView2Settings4,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, GetCookiesCompletedHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler, SourceChangedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW, SetTimer, SetWindowTextW,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_CAPTION,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{HRESULT, HSTRING, Interface, PCWSTR, PWSTR, w};

use crate::auth::{ORIGIN, Session};

const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";
const MUSIC_URL: &str = "https://music.youtube.com/";
const WINDOW_TITLE: &str = "Sign in to YouTube Music";
const WINDOW_SIZE: (i32, i32) = (520, 720);
const SILENT_TIMEOUT_MS: u32 = 20_000;
const TIMEOUT_TIMER: usize = 1;
const ALLOWED_DOMAINS: [&str; 2] = ["google.com", "youtube.com"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    // A visible window on Google's sign-in page.
    Interactive,
    // A hidden window that only reloads YouTube Music to pick up fresh cookies.
    Silent,
}

#[derive(Debug, Error)]
pub enum WebSignInError {
    #[error(
        "Sign-in needs Microsoft's 64-bit WebView2 Runtime, which could not start ({0}). Install the x64 \"Evergreen Standalone Installer\" from developer.microsoft.com/microsoft-edge/webview2 and try again."
    )]
    Runtime(String),
    #[error("the sign-in window failed: {0}")]
    WebView(String),
    #[error("the sign-in window was closed before signing in")]
    Cancelled,
    #[error("Google wants you to sign in again")]
    NeedsSignIn,
}

impl From<windows::core::Error> for WebSignInError {
    fn from(err: windows::core::Error) -> Self {
        Self::WebView(err.message())
    }
}

impl From<webview2_com::Error> for WebSignInError {
    fn from(err: webview2_com::Error) -> Self {
        Self::WebView(err.to_string())
    }
}

// Linked statically so the app stays a single exe without WebView2Loader.dll beside it.
#[link(name = "WebView2LoaderStatic", kind = "static")]
unsafe extern "system" {
    fn CreateCoreWebView2EnvironmentWithOptions(
        browser_folder: PCWSTR,
        user_data_folder: PCWSTR,
        options: *mut c_void,
        handler: *mut c_void,
    ) -> HRESULT;
}

type Outcome = Rc<RefCell<Option<Result<String, WebSignInError>>>>;

// Blocks until a signed-in cookie is read, the user closes the window, or (Silent) Google
// wants interaction. Must run on a thread of its own: it owns a COM apartment and a message loop.
pub fn cookie_from_sign_in(profile_dir: &Path, mode: Mode) -> Result<String, WebSignInError> {
    // SAFETY: initializes COM for this thread only; balanced by CoUninitialize below.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let result = run(profile_dir, mode);
    // SAFETY: every COM object created on this thread was dropped inside `run`.
    unsafe { CoUninitialize() };
    result
}

fn run(profile_dir: &Path, mode: Mode) -> Result<String, WebSignInError> {
    let window = Window::create()?;
    let environment = environment(profile_dir)?;
    let controller = Controller(controller(&environment, window.0)?);
    let webview = embed(&controller.0, window.0)?;
    let outcome = Outcome::default();
    restrict_navigation(&webview)?;
    show_host_in_title(&webview, window.0)?;
    deny_permissions_and_downloads(&webview)?;
    block_new_windows(&webview)?;
    watch_navigation(&webview, &outcome, mode)?;
    let start = match mode {
        Mode::Interactive => SIGN_IN_URL,
        Mode::Silent => MUSIC_URL,
    };
    // SAFETY: plain Win32/COM calls on the thread that owns the window and webview.
    unsafe {
        webview.Navigate(&HSTRING::from(start))?;
        match mode {
            Mode::Interactive => {
                let _ = ShowWindow(window.0, SW_SHOW);
            }
            Mode::Silent => {
                SetTimer(Some(window.0), TIMEOUT_TIMER, SILENT_TIMEOUT_MS, None);
            }
        }
    }
    pump(&outcome, window.0)
}

struct Window(HWND);

impl Window {
    fn create() -> Result<Self, WebSignInError> {
        let class = w!("zephyr-sign-in");
        // SAFETY: standard window creation; the class stays registered for the process lifetime.
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            });
            let scale = |size: i32| size * GetDpiForSystem() as i32 / 96;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(WINDOW_TITLE),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                scale(WINDOW_SIZE.0),
                scale(WINDOW_SIZE.1),
                None,
                None,
                Some(instance),
                None,
            )?;
            Ok(Self(hwnd))
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: fails harmlessly if the user already closed the window.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

struct Controller(ICoreWebView2Controller);

impl Drop for Controller {
    fn drop(&mut self) {
        // SAFETY: shuts the browser processes down before the window goes away.
        let _ = unsafe { self.0.Close() };
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        // SAFETY: ends this thread's message loop only.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: default handling for every other message.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn environment(profile_dir: &Path) -> Result<ICoreWebView2Environment, WebSignInError> {
    let profile = HSTRING::from(profile_dir.as_os_str());
    let created = Rc::new(RefCell::new(None));
    let slot = created.clone();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: `profile` and `handler` outlive the call; null options mean defaults.
            unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    PCWSTR(profile.as_ptr()),
                    std::ptr::null_mut(),
                    handler.as_raw(),
                )
            }
            .ok()
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, environment| {
            result?;
            *slot.borrow_mut() = environment;
            Ok(())
        }),
    )
    .map_err(|err| WebSignInError::Runtime(err.to_string()))?;
    created
        .take()
        .ok_or_else(|| WebSignInError::Runtime("no environment returned".into()))
}

fn controller(
    environment: &ICoreWebView2Environment,
    hwnd: HWND,
) -> Result<ICoreWebView2Controller, WebSignInError> {
    let created = Rc::new(RefCell::new(None));
    let slot = created.clone();
    let environment = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: plain COM call on the owning thread.
            unsafe { environment.CreateCoreWebView2Controller(hwnd, &handler) }
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, controller| {
            result?;
            *slot.borrow_mut() = controller;
            Ok(())
        }),
    )?;
    created
        .take()
        .ok_or_else(|| windows::core::Error::from(E_POINTER).into())
}

fn embed(
    controller: &ICoreWebView2Controller,
    hwnd: HWND,
) -> Result<ICoreWebView2, WebSignInError> {
    let mut bounds = RECT::default();
    // SAFETY: plain Win32/COM calls on the thread that owns the window and webview.
    unsafe {
        GetClientRect(hwnd, &mut bounds)?;
        controller.SetBounds(bounds)?;
        controller.SetIsVisible(true)?;
        let webview = controller.CoreWebView2()?;
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(false)?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetIsWebMessageEnabled(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        let settings = settings.cast::<ICoreWebView2Settings4>()?;
        settings.SetIsPasswordAutosaveEnabled(false)?;
        settings.SetIsGeneralAutofillEnabled(false)?;
        Ok(webview)
    }
}

// The window has no address bar, so it must never show a page outside Google/YouTube.
fn restrict_navigation(webview: &ICoreWebView2) -> Result<(), WebSignInError> {
    let handler = NavigationStartingEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut uri = PWSTR::null();
        // SAFETY: WebView2 allocates `uri`; take_pwstr frees it.
        unsafe {
            args.Uri(&mut uri)?;
            if !is_allowed_url(&take_pwstr(uri)) {
                args.SetCancel(true)?;
            }
        }
        Ok(())
    }));
    let mut token = 0;
    // SAFETY: plain COM call on the owning thread.
    unsafe { webview.add_NavigationStarting(&handler, &mut token)? };
    Ok(())
}

// Lets the user see which site they are typing into.
fn show_host_in_title(webview: &ICoreWebView2, hwnd: HWND) -> Result<(), WebSignInError> {
    let handler = SourceChangedEventHandler::create(Box::new(move |sender, _| {
        if let Some(webview) = sender {
            let url = source(&webview)?;
            let title = https_host(&url).map_or_else(
                || WINDOW_TITLE.to_owned(),
                |host| format!("{host} - {WINDOW_TITLE}"),
            );
            // SAFETY: plain Win32 call on the thread that owns the window.
            unsafe { SetWindowTextW(hwnd, &HSTRING::from(title))? };
        }
        Ok(())
    }));
    let mut token = 0;
    // SAFETY: plain COM call on the owning thread.
    unsafe { webview.add_SourceChanged(&handler, &mut token)? };
    Ok(())
}

fn deny_permissions_and_downloads(webview: &ICoreWebView2) -> Result<(), WebSignInError> {
    let permissions = PermissionRequestedEventHandler::create(Box::new(|_, args| match args {
        // SAFETY: plain COM call on the owning thread.
        Some(args) => unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) },
        None => Ok(()),
    }));
    let downloads = DownloadStartingEventHandler::create(Box::new(|_, args| match args {
        // SAFETY: plain COM call on the owning thread.
        Some(args) => unsafe { args.SetCancel(true) },
        None => Ok(()),
    }));
    let mut token = 0;
    // SAFETY: plain COM calls on the owning thread.
    unsafe {
        webview.add_PermissionRequested(&permissions, &mut token)?;
        webview
            .cast::<ICoreWebView2_4>()?
            .add_DownloadStarting(&downloads, &mut token)?;
    }
    Ok(())
}

// Links that would open a second window (help, create account) are ignored.
fn block_new_windows(webview: &ICoreWebView2) -> Result<(), WebSignInError> {
    let handler = NewWindowRequestedEventHandler::create(Box::new(|_, args| match args {
        // SAFETY: plain COM call on the owning thread.
        Some(args) => unsafe { args.SetHandled(true) },
        None => Ok(()),
    }));
    let mut token = 0;
    // SAFETY: plain COM call on the owning thread.
    unsafe { webview.add_NewWindowRequested(&handler, &mut token)? };
    Ok(())
}

fn watch_navigation(
    webview: &ICoreWebView2,
    outcome: &Outcome,
    mode: Mode,
) -> Result<(), WebSignInError> {
    let outcome = outcome.clone();
    let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, _| {
        let Some(webview) = sender else { return Ok(()) };
        if is_music_page(&source(&webview)?) {
            request_cookie(&webview, outcome.clone(), mode)
        } else {
            if mode == Mode::Silent {
                *outcome.borrow_mut() = Some(Err(WebSignInError::NeedsSignIn));
            }
            Ok(())
        }
    }));
    let mut token = 0;
    // SAFETY: plain COM call on the owning thread.
    unsafe { webview.add_NavigationCompleted(&handler, &mut token)? };
    Ok(())
}

fn source(webview: &ICoreWebView2) -> windows::core::Result<String> {
    let mut uri = PWSTR::null();
    // SAFETY: WebView2 allocates `uri`; take_pwstr frees it.
    unsafe { webview.Source(&mut uri)? };
    Ok(take_pwstr(uri))
}

fn request_cookie(
    webview: &ICoreWebView2,
    outcome: Outcome,
    mode: Mode,
) -> windows::core::Result<()> {
    let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
        result?;
        let cookie = cookie_header(list)?;
        if Session::from_cookie(&cookie).is_ok() {
            *outcome.borrow_mut() = Some(Ok(cookie));
        } else if mode == Mode::Silent {
            *outcome.borrow_mut() = Some(Err(WebSignInError::NeedsSignIn));
        }
        Ok(())
    }));
    // SAFETY: plain COM calls on the owning thread.
    unsafe {
        webview
            .cast::<ICoreWebView2_2>()?
            .CookieManager()?
            .GetCookies(&HSTRING::from(ORIGIN), &handler)
    }
}

fn cookie_header(list: Option<ICoreWebView2CookieList>) -> windows::core::Result<String> {
    let list = list.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
    let mut count = 0;
    // SAFETY: WebView2 allocates each name/value; take_pwstr frees them.
    unsafe {
        list.Count(&mut count)?;
        let pairs = (0..count)
            .map(|index| {
                let cookie = list.GetValueAtIndex(index)?;
                let (mut name, mut value) = (PWSTR::null(), PWSTR::null());
                cookie.Name(&mut name)?;
                cookie.Value(&mut value)?;
                Ok(format!("{}={}", take_pwstr(name), take_pwstr(value)))
            })
            .collect::<windows::core::Result<Vec<_>>>()?;
        Ok(pairs.join("; "))
    }
}

fn pump(outcome: &Outcome, hwnd: HWND) -> Result<String, WebSignInError> {
    let mut message = MSG::default();
    loop {
        if let Some(result) = outcome.take() {
            return result;
        }
        // SAFETY: standard message loop for windows owned by this thread.
        match unsafe { GetMessageW(&mut message, None, 0, 0) }.0 {
            -1 => return Err(windows::core::Error::from_thread().into()),
            0 => return Err(WebSignInError::Cancelled),
            _ if message.message == WM_TIMER
                && message.hwnd == hwnd
                && message.wParam.0 == TIMEOUT_TIMER =>
            {
                return Err(WebSignInError::NeedsSignIn);
            }
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
}

fn is_music_page(url: &str) -> bool {
    url.strip_prefix(ORIGIN)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

fn is_allowed_url(url: &str) -> bool {
    https_host(url).is_some_and(is_allowed_host)
}

fn is_allowed_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ALLOWED_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        || is_google_country_sign_in(&host)
}

// Google's sign-in also hops through accounts.google.<country> (e.g. .com.tr, .co.uk, .de).
fn is_google_country_sign_in(host: &str) -> bool {
    let Some(suffix) = host.strip_prefix("accounts.google.") else {
        return false;
    };
    let country = |label: &str| label.len() == 2 && label.bytes().all(|b| b.is_ascii_lowercase());
    match suffix.split('.').collect::<Vec<_>>().as_slice() {
        [code] => country(code),
        ["com" | "co", code] => country(code),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_youtube_music_pages_count_as_signed_in() {
        assert!(is_music_page("https://music.youtube.com"));
        assert!(is_music_page("https://music.youtube.com/"));
        assert!(is_music_page("https://music.youtube.com/library"));
        assert!(!is_music_page("https://music.youtube.com.evil.example/"));
        assert!(!is_music_page("http://music.youtube.com/"));
        assert!(!is_music_page(
            "https://accounts.google.com/?continue=https://music.youtube.com/"
        ));
    }

    #[test]
    fn allows_google_sign_in_hosts() {
        for url in [
            "https://accounts.google.com/ServiceLogin?continue=x",
            "https://accounts.youtube.com/accounts/SetSID",
            "https://music.youtube.com/",
            "https://myaccount.google.com/",
            "https://accounts.google.com.tr/accounts/SetSID",
            "https://accounts.google.co.uk/x",
            "https://accounts.google.de/x",
            "https://ACCOUNTS.GOOGLE.COM/x",
            "https://accounts.google.com:443/x",
        ] {
            assert!(is_allowed_url(url), "{url}");
        }
    }

    #[test]
    fn blocks_everything_else() {
        for url in [
            "http://accounts.google.com/",
            "https://evil.example/",
            "https://google.com.evil.example/",
            "https://accounts.google.com.evil.example/",
            "https://accounts.google.com@evil.example/",
            "https://notgoogle.com/",
            "https://accounts.google.us.com/",
            "https://www.google.evil/",
            "data:text/html,hi",
            "about:blank",
            "",
        ] {
            assert!(!is_allowed_url(url), "{url}");
        }
    }

    #[test]
    fn extracts_host_for_the_title() {
        assert_eq!(
            https_host("https://accounts.google.com/x?y#z"),
            Some("accounts.google.com")
        );
        assert_eq!(https_host("https://a.b:8443"), Some("a.b"));
        assert_eq!(https_host("http://a.b/"), None);
        assert_eq!(https_host("https://user@a.b/"), None);
    }
}
