//! Bounded framing and conversion between file paths and LSP coordinates.
use super::*;

/// The workspace/tab key for a path reported by the server.
pub(super) fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| normalize_path(path))
}

pub(super) fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let count = reader
            .take((HEADER_LIMIT + 1) as u64)
            .read_line(&mut line)?;
        if count == 0 {
            return if total == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete LSP header",
                ))
            };
        }
        total += count;
        if total > HEADER_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header is too large",
            ));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.trim_end().split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed LSP header",
            ));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate Content-Length",
                ));
            }
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length")
            })?);
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length == 0 || length > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds size limit",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(super) fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "LSP message is too large",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// The replacement range for incremental sync only needs the last line's UTF-16
/// width. Earlier lines can be counted as bytes without decoding every scalar.
pub(super) fn document_end_position(text: &str) -> Position {
    Position {
        line: text.bytes().filter(|byte| *byte == b'\n').count() as u32,
        character: text
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32,
    }
}

/// Convert egui's Unicode scalar offset to a zero-based UTF-16 LSP position.
pub fn char_to_position(text: &str, char_offset: usize) -> Position {
    let mut position = Position::default();
    let mut chars = text.chars().peekable();
    for _ in 0..char_offset {
        let Some(ch) = chars.next() else {
            break;
        };
        if ch == '\n' {
            position.line += 1;
            position.character = 0;
        } else if ch != '\r' || chars.peek() != Some(&'\n') {
            position.character += ch.len_utf16() as u32;
        }
    }
    position
}

/// Clamp stale positions to their line end/document end. A position inside a
/// surrogate pair clamps to the beginning of the corresponding Unicode scalar.
pub fn position_to_char(text: &str, position: Position) -> usize {
    let mut line = 0;
    let mut column = 0;
    let mut index = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if line == position.line {
            if column >= position.character
                || ch == '\n'
                || (ch == '\r' && chars.peek() == Some(&'\n'))
            {
                return index;
            }
            let units = ch.len_utf16() as u32;
            if column + units > position.character {
                return index;
            }
            column += units;
        }
        index += 1;
        if ch == '\n' {
            line += 1;
            column = 0;
        }
    }
    index
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{rest}"));
    }
    let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    #[cfg(windows)]
    {
        let mut text = text.to_owned();
        if text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic() {
            let drive = (text.as_bytes()[0] as char)
                .to_ascii_uppercase()
                .to_string();
            text.replace_range(0..1, &drive);
        }
        PathBuf::from(text)
    }
    #[cfg(not(windows))]
    PathBuf::from(text)
}

pub(super) fn path_to_uri(path: &Path) -> Result<String, String> {
    let path = normalize_path(path);
    let text = path
        .to_str()
        .ok_or("Language server cannot represent this file path as Unicode")?
        .replace('\\', "/");
    let windows_drive =
        text.as_bytes().get(1) == Some(&b':') && text.as_bytes().get(2) == Some(&b'/');
    let (prefix, path) = if let Some(unc) = text.strip_prefix("//") {
        ("file://", unc)
    } else if windows_drive {
        ("file:///", text.as_str())
    } else if text.starts_with('/') {
        ("file://", text.as_str())
    } else {
        return Err("Language server requires an absolute file path".into());
    };
    let mut uri = String::from(prefix);
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    Ok(uri)
}

pub(super) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let source = encoded.as_bytes();
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' {
            let digits = source.get(index + 1..index + 3)?;
            let hex = std::str::from_utf8(digits).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(source[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded.contains('\0') {
        return None;
    }
    let decoded = if let Some(path) = decoded.strip_prefix("localhost/") {
        format!("/{path}")
    } else {
        decoded
    };
    let decoded = if let Some(stripped) = decoded.strip_prefix('/') {
        if decoded.as_bytes().get(2) == Some(&b':') {
            stripped.to_owned()
        } else {
            decoded
        }
    } else {
        format!("//{decoded}")
    };
    #[cfg(windows)]
    let decoded = decoded.replace('/', "\\");
    Some(normalize_path(Path::new(&decoded)))
}
