//! Small helpers shared across modules.

/// Convert a Rust string into a NUL-terminated UTF-16 buffer for Win32 `*W` APIs.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
