//! Offline statistics for the focused buffer. Copy and tokenize only after a
//! quiet interval, with at most one worker and one retained text snapshot.
use eframe::egui;
use std::{
    cell::RefCell,
    ops::Range,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Document {
    id: u64,
    revision: u64,
    bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    document: Document,
    selection: Option<Range<usize>>,
}

struct Tokens {
    file: usize,
    selection: Option<usize>,
}

struct Running {
    key: Key,
    receiver: mpsc::Receiver<Tokens>,
}

#[derive(Default)]
struct Cache {
    key: Option<Key>,
    due: Option<Instant>,
    text: Option<Arc<str>>,
    file: Option<usize>,
    selection: Option<usize>,
    error: Option<String>,
    running: Option<Running>,
}

#[derive(Default)]
pub struct Statistics(RefCell<Cache>);

pub struct Display {
    pub file_chars: usize,
    pub selected_chars: Option<usize>,
    file_tokens: Option<usize>,
    selected_tokens: Option<usize>,
    error: Option<String>,
}

impl Statistics {
    #[allow(clippy::too_many_arguments)]
    pub fn display(
        &self,
        ctx: &egui::Context,
        id: u64,
        revision: u64,
        text: &str,
        chars: usize,
        selection: Range<usize>,
    ) -> Display {
        self.display_at(ctx, id, revision, text, chars, selection, Instant::now())
    }

    #[allow(clippy::too_many_arguments)]
    fn display_at(
        &self,
        ctx: &egui::Context,
        id: u64,
        revision: u64,
        text: &str,
        chars: usize,
        selection: Range<usize>,
        now: Instant,
    ) -> Display {
        let start = selection.start.min(selection.end).min(chars);
        let end = selection.start.max(selection.end).min(chars);
        let key = Key {
            document: Document {
                id,
                revision,
                bytes: text.len(),
            },
            selection: (start < end).then_some(start..end),
        };
        let mut cache = self.0.borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            if cache.key.as_ref().map(|key| key.document) != Some(key.document) {
                cache.text = None;
                cache.file = text.is_empty().then_some(0);
            }
            cache.selection = None;
            cache.error = None;
            cache.due = Some(now + DEBOUNCE);
            cache.key = Some(key.clone());
        }
        cache.poll();
        if cache.needs_tokens() && cache.error.is_none() {
            let delay = cache.due.unwrap_or(now).saturating_duration_since(now);
            if !delay.is_zero() {
                ctx.request_repaint_after(delay);
            } else if cache.running.is_none() {
                // Selection/cursor movement reuses this snapshot and the full
                // document count. No text copy occurs during the debounce.
                let snapshot = cache.text.get_or_insert_with(|| Arc::from(text)).clone();
                let file = cache.file;
                let worker_key = key.clone();
                let worker_ctx = ctx.clone();
                let (sender, receiver) = mpsc::sync_channel(1);
                match std::thread::Builder::new()
                    .name("vedit-tokens".into())
                    .spawn(move || {
                        // Also wake the UI on an unexpected tokenizer failure, so
                        // the disconnected channel becomes an explicit error.
                        let result = std::panic::catch_unwind(|| {
                            count_tokens(&snapshot, worker_key.selection, file)
                        });
                        if let Ok(tokens) = result {
                            let _ = sender.send(tokens);
                        }
                        drop(sender);
                        worker_ctx.request_repaint();
                    }) {
                    Ok(_) => {
                        cache.running = Some(Running {
                            key: key.clone(),
                            receiver,
                        })
                    }
                    Err(error) => {
                        cache.error = Some(format!("Could not start token counting: {error}"))
                    }
                }
            }
        }
        Display {
            file_chars: chars,
            selected_chars: key.selection.map(|range| range.len()),
            file_tokens: cache.file,
            selected_tokens: cache.selection,
            error: cache.error.clone(),
        }
    }

    pub fn clear(&self) {
        let mut cache = self.0.borrow_mut();
        cache.key = None;
        cache.text = None;
        cache.file = None;
        cache.selection = None;
        cache.error = None;
        // Retain the receiver until completion to keep the one-worker bound
        // even when the last tab is closed and another is immediately opened.
        cache.poll();
    }

    #[cfg(test)]
    pub fn pending(&self) -> bool {
        let cache = self.0.borrow();
        cache.error.is_none() && cache.needs_tokens()
    }
}

impl Cache {
    fn needs_tokens(&self) -> bool {
        self.key.as_ref().is_some_and(|key| {
            self.file.is_none() || (key.selection.is_some() && self.selection.is_none())
        })
    }

    fn poll(&mut self) {
        let Some(running) = &self.running else { return };
        let result = match running.receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return,
            result => result,
        };
        let running = self.running.take().unwrap();
        if self.key.as_ref().map(|key| key.document) != Some(running.key.document) {
            return;
        }
        match result {
            Ok(tokens) => {
                self.file = Some(tokens.file);
                if self.key.as_ref() == Some(&running.key) {
                    self.selection = tokens.selection;
                }
            }
            Err(_) if self.key.as_ref() == Some(&running.key) => {
                self.error =
                    Some("Token counting failed. Reopen the file or edit to retry.".into());
            }
            Err(_) => {}
        }
    }
}

fn count_tokens(text: &str, selection: Option<Range<usize>>, file: Option<usize>) -> Tokens {
    let tokenizer = tiktoken_rs::o200k_base_singleton();
    // Source text is always ordinary text, even if it contains strings such
    // as <|endoftext|>. They must not become tokenizer control tokens.
    let file = file.unwrap_or_else(|| tokenizer.encode_ordinary(text).len());
    let selection = selection.map(|range| {
        let start = text
            .char_indices()
            .nth(range.start)
            .map_or(text.len(), |(byte, _)| byte);
        let end = text[start..]
            .char_indices()
            .nth(range.len())
            .map_or(text.len(), |(byte, _)| start + byte);
        if start == 0 && end == text.len() {
            file
        } else {
            tokenizer.encode_ordinary(&text[start..end]).len()
        }
    });
    Tokens { file, selection }
}

impl Display {
    pub fn label(&self, compact: bool) -> String {
        let (prefix, chars, tokens) = if let Some(chars) = self.selected_chars {
            (
                if compact { "Sel: " } else { "Selection: " },
                chars,
                self.selected_tokens,
            )
        } else {
            ("", self.file_chars, self.file_tokens)
        };
        let chars = if compact {
            abbreviated(chars)
        } else {
            grouped(chars)
        };
        let tokens = match tokens {
            Some(tokens) => format!("≈{}", abbreviated(tokens)),
            None if self.error.is_some() => "—".into(),
            None => "…".into(),
        };
        if compact {
            format!("{prefix}{chars} ch · {tokens} tok")
        } else {
            format!("{prefix}{chars} chars · {tokens} tokens")
        }
    }

    pub fn tooltip(&self) -> String {
        let token_text = |tokens: Option<usize>| {
            tokens.map(grouped).unwrap_or_else(|| {
                if self.error.is_some() {
                    "unavailable".into()
                } else {
                    "counting…".into()
                }
            })
        };
        let mut text = format!(
            "File: {} characters · {} text tokens",
            grouped(self.file_chars),
            token_text(self.file_tokens)
        );
        if let Some(chars) = self.selected_chars {
            text.push_str(&format!(
                "\nSelection: {} characters · {} text tokens",
                grouped(chars),
                token_text(self.selected_tokens)
            ));
        }
        text.push_str("\n\nCurrent editor text, including unsaved changes.\nCharacters: Unicode codepoints, including spaces and line breaks.\nCombined emoji may contain several codepoints; raw CRLF counts as two.\n\nTokens: o200k_base, counted locally and offline.\nExact for this tokenizer; ≈ is a reference for other models.\nSelection is counted as a standalone excerpt.\nAgent instructions, conversation and tool data are not included.");
        if let Some(error) = &self.error {
            text.push_str(&format!("\n\n{error}"));
        }
        text
    }
}

fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(ch);
    }
    output
}

fn abbreviated(value: usize) -> String {
    if value >= 999_950 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1000 {
        format!("{:.1}k", value as f64 / 1000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_counts_include_unicode_whitespace_and_literal_special_tokens() {
        for (text, expected) in [
            ("", 0),
            ("tiktoken is great!", 6),
            ("2 + 2 = 4", 7),
            ("お誕生日おめでとう", 8),
        ] {
            assert_eq!(count_tokens(text, None, None).file, expected, "{text}");
        }
        let text = "ä👩‍💻\r\n<|endoftext|>";
        let result = count_tokens(text, Some(1..4), None);
        assert_eq!(result.selection, Some(count_tokens("👩‍💻", None, None).file));
        assert_eq!(count_tokens(text, Some(4..6), None).selection, Some(1));
        assert!(count_tokens("<|endoftext|>", None, None).file > 1);
        assert_eq!(
            count_tokens(text, Some(0..text.chars().count()), None).selection,
            Some(result.file)
        );
        let index = crate::render::LineIndex::default();
        assert_eq!(index.char_count("ä👩‍💻\r\n", 0), 6);
        assert_eq!(index.char_count("", 1), 0);
    }

    #[test]
    fn debounce_does_not_copy_text_and_cursor_motion_keeps_cached_tokens() {
        let stats = Statistics::default();
        let ctx = egui::Context::default();
        let now = Instant::now();
        let text = "tiktoken is great!";
        let show =
            |time, selection| stats.display_at(&ctx, 1, 0, text, text.len(), selection, time);
        assert_eq!(show(now, 0..0).file_tokens, None);
        show(now + Duration::from_millis(249), 4..4);
        assert!(stats.0.borrow().text.is_none());
        assert!(stats.0.borrow().running.is_none());
        show(now + DEBOUNCE, 4..4);
        assert!(stats.0.borrow().running.is_some());
        let deadline = Instant::now() + Duration::from_secs(15);
        while show(now + DEBOUNCE, 4..4).file_tokens.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let snapshot = stats.0.borrow().text.clone().unwrap();
        assert_eq!(show(now + DEBOUNCE, 12..12).file_tokens, Some(6));
        assert!(stats.0.borrow().running.is_none());
        let selected = show(now + DEBOUNCE, Range { start: 8, end: 0 });
        assert_eq!(selected.selected_chars, Some(8));
        assert_eq!(selected.file_tokens, Some(6));
        assert!(Arc::ptr_eq(
            &snapshot,
            stats.0.borrow().text.as_ref().unwrap()
        ));
    }

    fn queued(key: Key, current: Key) -> (Cache, mpsc::SyncSender<Tokens>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            Cache {
                key: Some(current),
                running: Some(Running { key, receiver }),
                ..Cache::default()
            },
            sender,
        )
    }

    #[test]
    fn stale_document_revision_and_selection_results_cannot_replace_current_values() {
        let key = Key {
            document: Document {
                id: 1,
                revision: 3,
                bytes: 12,
            },
            selection: Some(1..4),
        };
        for current in [
            Key {
                document: Document {
                    id: 2,
                    ..key.document
                },
                ..key.clone()
            },
            Key {
                document: Document {
                    revision: 4,
                    ..key.document
                },
                ..key.clone()
            },
        ] {
            let (mut cache, sender) = queued(key.clone(), current);
            sender
                .send(Tokens {
                    file: 6,
                    selection: Some(2),
                })
                .unwrap();
            cache.poll();
            assert!(cache.file.is_none());
            assert!(cache.selection.is_none());
            assert!(cache.running.is_none());
        }
        let (mut cache, sender) = queued(
            key.clone(),
            Key {
                selection: Some(5..8),
                ..key
            },
        );
        sender
            .send(Tokens {
                file: 6,
                selection: Some(2),
            })
            .unwrap();
        cache.poll();
        assert_eq!(cache.file, Some(6));
        assert!(cache.selection.is_none());
        assert!(cache.needs_tokens());
    }

    #[test]
    fn closed_tabs_release_snapshot_and_failed_worker_is_not_a_permanent_spinner() {
        let key = Key {
            document: Document {
                id: 1,
                revision: 0,
                bytes: 4,
            },
            selection: None,
        };
        let (cache, sender) = queued(key.clone(), key.clone());
        let stats = Statistics(RefCell::new(cache));
        stats.clear();
        assert!(stats.0.borrow().running.is_some());
        sender
            .send(Tokens {
                file: 1,
                selection: None,
            })
            .unwrap();
        stats.clear();
        assert!(stats.0.borrow().file.is_none());
        assert!(stats.0.borrow().running.is_none());
        let (mut cache, sender) = queued(key.clone(), key);
        drop(sender);
        cache.poll();
        assert!(cache.error.is_some());
        assert!(cache.running.is_none());
    }

    #[test]
    fn display_names_selection_and_preserves_exact_file_totals_in_tooltip() {
        let display = Display {
            file_chars: 12_480,
            selected_chars: Some(640),
            file_tokens: Some(3214),
            selected_tokens: Some(180),
            error: None,
        };
        assert_eq!(display.label(false), "Selection: 640 chars · ≈180 tokens");
        assert_eq!(display.label(true), "Sel: 640 ch · ≈180 tok");
        assert!(
            display
                .tooltip()
                .contains("File: 12,480 characters · 3,214 text tokens")
        );
        assert!(
            display
                .tooltip()
                .contains("Selection: 640 characters · 180 text tokens")
        );
        assert!(display.tooltip().contains("o200k_base"));
        assert_eq!(abbreviated(3214), "3.2k");
        assert_eq!(abbreviated(999_999), "1.0m");
    }
}
