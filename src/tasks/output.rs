//! Concurrent pipe draining, bounded output, and incremental UTF-8 decoding.

use super::*;

pub(super) fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    sender: SyncSender<TaskEvent>,
    budget: Arc<Mutex<OutputBudget>>,
    cancel_requested: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("vedit-task-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut decoder = Utf8Decoder::default();
            let mut truncated = false;
            loop {
                let text = match pipe.read(&mut buffer) {
                    Ok(0) => decoder.finish(),
                    // The shared budget never reopens. Keep draining the pipe
                    // so the command can finish, but avoid decoding/allocating
                    // every later chunk once this reader observes truncation.
                    Ok(_) if truncated => continue,
                    Ok(count) => decoder.decode(&buffer[..count]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        if !cancel_requested.load(Ordering::Acquire) {
                            let _ = send_output(
                                &sender,
                                TaskEvent::Error(format!("Could not read command output: {error}")),
                                &cancel_requested,
                            );
                        }
                        break;
                    }
                };
                let eof = text.1;
                if !text.0.is_empty() {
                    let mut budget = budget
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let chunks = budget.take(&text.0);
                    truncated = budget.truncated;
                    for chunk in chunks {
                        if !send_output(&sender, TaskEvent::Output(chunk), &cancel_requested) {
                            return;
                        }
                    }
                }
                if eof {
                    break;
                }
            }
        })
}

fn send_output(
    sender: &SyncSender<TaskEvent>,
    mut event: TaskEvent,
    cancel_requested: &AtomicBool,
) -> bool {
    loop {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(unsent)) => {
                if cancel_requested.load(Ordering::Acquire) {
                    return false;
                }
                event = unsent;
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct OutputBudget {
    streaming: bool,
    bytes: usize,
    newlines: usize,
    truncated: bool,
}

impl OutputBudget {
    pub(super) fn new(streaming: bool) -> Self {
        Self {
            streaming,
            ..Self::default()
        }
    }

    pub(super) fn take(&mut self, text: &str) -> Vec<String> {
        if self.streaming {
            return vec![text.to_owned()];
        }
        if self.truncated || text.is_empty() {
            return Vec::new();
        }
        let bytes_left = (MAX_OUTPUT_BYTES - TRUNCATION_NOTICE.len()).saturating_sub(self.bytes);
        let lines_left = (MAX_OUTPUT_LINES - 2).saturating_sub(self.newlines);
        let mut end = 0;
        let mut newlines = 0;
        for (index, character) in text.char_indices() {
            let next = index + character.len_utf8();
            if next > bytes_left || (character == '\n' && newlines == lines_left) {
                break;
            }
            end = next;
            newlines += usize::from(character == '\n');
        }
        let mut chunks = Vec::with_capacity(2);
        if end > 0 {
            chunks.push(text[..end].to_owned());
            self.bytes += end;
            self.newlines += newlines;
        }
        if end < text.len() {
            self.truncated = true;
            chunks.push(TRUNCATION_NOTICE.to_owned());
            self.bytes += TRUNCATION_NOTICE.len();
            self.newlines += 2;
        }
        chunks
    }
}

#[derive(Default)]
pub(super) struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub(super) fn decode(&mut self, bytes: &[u8]) -> (String, bool) {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut offset = 0;
        while offset < self.pending.len() {
            match std::str::from_utf8(&self.pending[offset..]) {
                Ok(valid) => {
                    text.push_str(valid);
                    offset = self.pending.len();
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    // The decoder has verified that this prefix is valid.
                    text.push_str(
                        std::str::from_utf8(&self.pending[offset..valid_end]).unwrap_or_default(),
                    );
                    match error.error_len() {
                        Some(length) => {
                            text.push('\u{fffd}');
                            offset = valid_end + length;
                        }
                        None => {
                            offset = valid_end;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..offset);
        (text, false)
    }

    pub(super) fn finish(&mut self) -> (String, bool) {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        (text, true)
    }
}
