use crate::application::clipboard::ClipboardErrorCode;

#[cfg(windows)]
pub fn read_clipboard_text() -> Result<String, ClipboardErrorCode> {
    use crate::application::clipboard::normalize_clipboard_link;
    use windows_sys::Win32::{
        Foundation::HGLOBAL,
        System::{
            DataExchange::{
                CloseClipboard, CountClipboardFormats, GetClipboardData,
                IsClipboardFormatAvailable, OpenClipboard,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        },
    };
    const CF_UNICODETEXT: u32 = 13;
    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    struct MemoryGuard(HGLOBAL);
    impl Drop for MemoryGuard {
        fn drop(&mut self) {
            unsafe {
                GlobalUnlock(self.0);
            }
        }
    }
    // Clipboard ownership and global-memory locks remain held for the bounded copy only.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err(ClipboardErrorCode::Unavailable);
        }
        let _clipboard = ClipboardGuard;
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return Err(if CountClipboardFormats() == 0 {
                ClipboardErrorCode::Empty
            } else {
                ClipboardErrorCode::UnsupportedContent
            });
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            return Err(ClipboardErrorCode::Unavailable);
        }
        let bytes = GlobalSize(handle);
        if bytes > 16_384 {
            return Err(ClipboardErrorCode::TooLong);
        }
        if bytes < 2 || !bytes.is_multiple_of(2) {
            return Err(ClipboardErrorCode::InvalidText);
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return Err(ClipboardErrorCode::Unavailable);
        }
        let _memory = MemoryGuard(handle);
        let units = std::slice::from_raw_parts(pointer.cast::<u16>(), bytes / 2);
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(ClipboardErrorCode::InvalidText)?;
        let text =
            String::from_utf16(&units[..end]).map_err(|_| ClipboardErrorCode::InvalidText)?;
        normalize_clipboard_link(&text)
    }
}

#[cfg(not(windows))]
pub fn read_clipboard_text() -> Result<String, ClipboardErrorCode> {
    Err(ClipboardErrorCode::Unavailable)
}
