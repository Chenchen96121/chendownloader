use local_media_downloader_lib::application::clipboard::{
    normalize_clipboard_link, ClipboardErrorCode,
};

#[test]
fn accepts_trimmed_single_line_without_parsing_or_extracting_urls() {
    assert_eq!(
        normalize_clipboard_link("  https://youtu.be/91YrsgWQcEo  "),
        Ok("https://youtu.be/91YrsgWQcEo".into())
    );
    assert_eq!(
        normalize_clipboard_link("not a url"),
        Ok("not a url".into())
    );
}
#[test]
fn rejects_empty_multiline_control_and_overlong_text_without_truncation() {
    for (input, expected) in [
        ("", ClipboardErrorCode::Empty),
        ("  ", ClipboardErrorCode::Empty),
        ("a\nb", ClipboardErrorCode::InvalidText),
        ("a\0b", ClipboardErrorCode::InvalidText),
        ("a\rb", ClipboardErrorCode::InvalidText),
        ("a\u{2028}b", ClipboardErrorCode::InvalidText),
        ("a\tb", ClipboardErrorCode::InvalidText),
    ] {
        assert_eq!(normalize_clipboard_link(input), Err(expected));
    }
    assert_eq!(
        normalize_clipboard_link(&"x".repeat(2049)),
        Err(ClipboardErrorCode::TooLong)
    );
    assert_eq!(
        normalize_clipboard_link(&"中".repeat(683)),
        Err(ClipboardErrorCode::TooLong)
    );
    assert_eq!(
        normalize_clipboard_link(&"😀".repeat(1025)),
        Err(ClipboardErrorCode::TooLong)
    );
    assert_eq!(
        normalize_clipboard_link(&"x".repeat(2048)).unwrap().len(),
        2048
    );
}
