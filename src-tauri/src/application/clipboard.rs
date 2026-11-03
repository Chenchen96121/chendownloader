#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardErrorCode {
    Empty,
    UnsupportedContent,
    TooLong,
    InvalidText,
    Unavailable,
}

pub fn normalize_clipboard_link(text: &str) -> Result<String, ClipboardErrorCode> {
    if text
        .chars()
        .any(|ch| matches!(ch, '\0' | '\r' | '\n' | '\u{2028}' | '\u{2029}'))
    {
        return Err(ClipboardErrorCode::InvalidText);
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(ClipboardErrorCode::Empty);
    }
    if text.len() > 2048 || text.encode_utf16().count() > 2048 {
        return Err(ClipboardErrorCode::TooLong);
    }
    if text.chars().any(char::is_control) {
        return Err(ClipboardErrorCode::InvalidText);
    }
    Ok(text.to_owned())
}
