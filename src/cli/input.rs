//! Bounded external input decoding and request path resolution.
use super::*;

pub(super) fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
pub(super) fn read_json_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1_048_576 {
        return Err("Input file exceeds 1 MiB; use smaller range edits or requests".into());
    }
    decode_input(&bytes).map(String::into_bytes)
}
pub(super) fn json_bytes(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

pub(super) fn read_input_file(path: &std::path::Path) -> Result<String, String> {
    String::from_utf8(read_json_file(path)?).map_err(|error| error.to_string())
}

pub(super) fn decode_input(bytes: &[u8]) -> Result<String, String> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let big_endian = bytes[0] == 0xfe;
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(
                "Incomplete UTF-16 input; save the complete file as UTF-8 or UTF-16 with BOM"
                    .into(),
            );
        }
        let units: Vec<_> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                let pair = [pair[0], pair[1]];
                if big_endian {
                    u16::from_be_bytes(pair)
                } else {
                    u16::from_le_bytes(pair)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| {
            "Invalid UTF-16 input; save the file as valid UTF-8 or UTF-16 with BOM".to_owned()
        })?
    } else {
        std::str::from_utf8(json_bytes(bytes))
            .map_err(|_| "Input is not UTF-8; save it as UTF-8 or UTF-16 with BOM".to_owned())?
            .to_owned()
    };
    if text.len() > 1_048_576 {
        return Err("Decoded input exceeds 1 MiB; use smaller range edits or requests".into());
    }
    Ok(text)
}

pub(super) fn resolve_paths(request: &mut Request) {
    match request {
        Request::Open { files, .. } | Request::Review { files, .. } => {
            for file in files {
                file.path = absolute(file.path.clone());
            }
        }
        Request::Workspace { path } => *path = absolute(path.clone()),
        Request::New {
            path: Some(path), ..
        }
        | Request::SaveAs { path, .. } => *path = absolute(path.clone()),
        Request::File { action } => match action {
            FileAction::Rename { path, .. } | FileAction::Delete { path } => {
                *path = absolute(path.clone())
            }
            FileAction::CreateFolder { parent, .. } => *parent = absolute(parent.clone()),
        },
        Request::Search {
            workspace: Some(path),
            ..
        }
        | Request::GitBranches {
            workspace: Some(path),
            ..
        }
        | Request::GitHistory {
            workspace: Some(path),
            ..
        }
        | Request::GitSwitchBranch {
            workspace: Some(path),
            ..
        }
        | Request::GitStatus {
            workspace: Some(path),
            ..
        }
        | Request::ChangeReview {
            workspace: Some(path),
            ..
        }
        | Request::Launch {
            workspace: Some(path),
            ..
        }
        | Request::Task {
            workspace: Some(path),
            ..
        }
        | Request::Output {
            workspace: Some(path),
            ..
        } => *path = absolute(path.clone()),
        Request::Notify {
            workspace, open, ..
        } => {
            if let Some(path) = workspace {
                *path = absolute(path.clone());
            }
            if let Some(open) = open {
                open.path = absolute(open.path.clone());
            }
        }
        Request::Batch { actions, .. } => {
            for action in actions {
                resolve_paths(action);
            }
        }
        _ => {}
    }
}
