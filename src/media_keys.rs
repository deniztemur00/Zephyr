use iced::futures::Stream;
use iced::futures::channel::mpsc;

const EVENT_BUFFER: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
}

impl MediaKey {
    const ALL: [MediaKey; 3] = [MediaKey::PlayPause, MediaKey::Next, MediaKey::Previous];

    fn id(self) -> i32 {
        self as i32 + 1
    }

    fn from_id(id: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.id() as usize == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Pressed(MediaKey),
    Unavailable(Vec<MediaKey>),
}

pub fn listen() -> impl Stream<Item = Event> {
    let (sender, receiver) = mpsc::channel(EVENT_BUFFER);
    std::thread::spawn(move || platform::run(sender));
    receiver
}

#[cfg(windows)]
mod platform {
    use std::ptr;

    use std::thread;
    use std::time::Duration;

    use iced::futures::channel::mpsc::Sender;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VIRTUAL_KEY, VK_MEDIA_NEXT_TRACK,
        VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, WM_HOTKEY,
    };

    use super::{Event, MediaKey};

    const POLL_MS: u32 = 250;
    const REGISTER_ATTEMPTS: u32 = 10;
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    fn virtual_key(key: MediaKey) -> VIRTUAL_KEY {
        match key {
            MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
            MediaKey::Next => VK_MEDIA_NEXT_TRACK,
            MediaKey::Previous => VK_MEDIA_PREV_TRACK,
        }
    }

    // Hotkeys registered without a window are delivered to this thread and to no other app.
    fn register(key: MediaKey) -> bool {
        // SAFETY: plain Win32 call; a null HWND binds the hotkey to this thread's queue.
        let ok = unsafe {
            RegisterHotKey(
                ptr::null_mut(),
                key.id(),
                MOD_NOREPEAT,
                u32::from(virtual_key(key)),
            )
        };
        ok != 0
    }

    // A just-disabled listener may hold the keys for up to POLL_MS while it shuts down.
    fn register_with_retry(key: MediaKey) -> bool {
        (0..REGISTER_ATTEMPTS).any(|attempt| {
            if attempt > 0 {
                thread::sleep(RETRY_DELAY);
            }
            register(key)
        })
    }

    pub fn run(mut sender: Sender<Event>) {
        let failed: Vec<MediaKey> = MediaKey::ALL
            .into_iter()
            .filter(|&key| !register_with_retry(key))
            .collect();
        if !failed.is_empty() {
            let _ = sender.try_send(Event::Unavailable(failed));
        }
        // SAFETY: MSG is plain data; all-zero is a valid initial value.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // The app drops the receiver when the setting is turned off; then release the keys.
        while !sender.is_closed() {
            // SAFETY: no handles are passed; this only waits for input or the timeout.
            unsafe { MsgWaitForMultipleObjects(0, ptr::null(), 0, POLL_MS, QS_ALLINPUT) };
            // SAFETY: `message` outlives each call.
            while unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                let key = (message.message == WM_HOTKEY)
                    .then(|| MediaKey::from_id(message.wParam))
                    .flatten();
                if let Some(key) = key {
                    let _ = sender.try_send(Event::Pressed(key));
                }
            }
        }
        for key in MediaKey::ALL {
            // SAFETY: unregistering an id this thread never registered is a harmless no-op.
            unsafe { UnregisterHotKey(ptr::null_mut(), key.id()) };
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use iced::futures::channel::mpsc::Sender;

    use super::Event;

    pub fn run(_sender: Sender<Event>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_ids_round_trip() {
        for key in MediaKey::ALL {
            assert_eq!(MediaKey::from_id(key.id() as usize), Some(key));
        }
        assert_eq!(MediaKey::from_id(0), None);
        assert_eq!(MediaKey::from_id(99), None);
    }
}
