//! Lossless conversion between local paths and LSP file URIs.

use std::path::Path;

use lsp_types::Uri;

#[must_use]
pub fn file_uri(path: &Path) -> Option<Uri> {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@' => uri.push(*byte as char),
            other => {
                use std::fmt::Write as _;
                let _ = write!(uri, "%{other:02X}");
            }
        }
    }
    uri.parse().ok()
}

/// The filesystem path of a `file://` URI.
#[must_use]
pub fn to_path(uri: &Uri) -> String {
    uri.path().decode().to_string_lossy().into_owned()
}
