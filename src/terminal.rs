//! Project-owned interactive shells. Views can move without moving their process.
//!
//! Alacritty owns VT interpretation, screen/scrollback and selections. The PTY
//! worker owns all process and pipe I/O; the widget only reads a screen snapshot
//! and sends input. Dropping this handle requests teardown of its process tree.

use alacritty_terminal::{
    event::{Event as TerminalEvent, EventListener, WindowSize},
    grid::Scroll,
    term::{Config, Term, TermMode, test::TermSize},
    vte::ansi::{Processor, Rgb},
};
use eframe::egui;
use portable_pty::PtySize;
use std::{
    ffi::OsString,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
};

mod input;
mod widget;
mod worker;

const INITIAL_COLUMNS: u16 = 80;
const INITIAL_ROWS: u16 = 24;
const MAX_HISTORY_LINES: usize = 10_000;
const MAX_HISTORY_CELLS: usize = 1_000_000;

enum Command {
    Write(Vec<u8>),
    Resize(PtySize),
    Stop,
    Failed(String),
}

struct State {
    title: String,
    status: String,
    running: bool,
    size: PtySize,
}

#[derive(Clone)]
struct Proxy {
    commands: mpsc::Sender<Command>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
}

impl EventListener for Proxy {
    fn send_event(&self, event: TerminalEvent) {
        match event {
            TerminalEvent::PtyWrite(text) => {
                let _ = self.commands.send(Command::Write(text.into_bytes()));
            }
            TerminalEvent::Title(title) => {
                self.state.lock().unwrap().title = title
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(160)
                    .collect();
            }
            TerminalEvent::ResetTitle => self.state.lock().unwrap().title = "PowerShell".into(),
            TerminalEvent::ColorRequest(index, formatter) => {
                let rgb = widget::default_color(index);
                let _ = self
                    .commands
                    .send(Command::Write(formatter(rgb).into_bytes()));
            }
            TerminalEvent::TextAreaSizeRequest(formatter) => {
                let size = self.state.lock().unwrap().size;
                let text = formatter(WindowSize {
                    num_lines: size.rows,
                    num_cols: size.cols,
                    cell_width: size.pixel_width / size.cols.max(1),
                    cell_height: size.pixel_height / size.rows.max(1),
                });
                let _ = self.commands.send(Command::Write(text.into_bytes()));
            }
            TerminalEvent::ClipboardStore(_, text) => self.ctx.copy_text(text),
            // Clipboard reads remain disabled by Alacritty's default OSC52
            // policy. Keyboard/menu paste is an explicit user action.
            _ => {}
        }
        self.ctx.request_repaint();
    }
}

pub(crate) struct TerminalSession {
    term: Arc<Mutex<Term<Proxy>>>,
    commands: mpsc::Sender<Command>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
    size: PtySize,
    preedit: String,
    scroll_rows: f32,
    #[cfg(test)]
    test_commands: Option<mpsc::Receiver<Command>>,
    #[cfg(test)]
    test_processor: Processor,
}

impl TerminalSession {
    /// Start asynchronously. Environment resolution, directory validation,
    /// shell discovery, profile setup and ConPTY creation happen on the worker.
    pub(crate) fn spawn(
        root: &Path,
        history_dir: &Path,
        environment: impl FnOnce() -> Vec<(OsString, OsString)> + Send + 'static,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let (session, receiver) = Self::new(ctx);
        let root = root.to_owned();
        let history_dir = history_dir.to_owned();
        let term = session.term.clone();
        let state = session.state.clone();
        let commands = session.commands.clone();
        let context = session.ctx.clone();
        thread::Builder::new()
            .name("vedit-terminal".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker::run(
                        worker::ShellSpec {
                            root,
                            history_dir,
                            environment: environment(),
                        },
                        receiver,
                        commands,
                        term,
                        &state,
                        &context,
                    )
                }));
                let error = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error,
                    Err(_) => "Terminal worker stopped unexpectedly".into(),
                };
                let mut state = state.lock().unwrap();
                state.running = false;
                state.status = format!("Failed: {error}");
                context.request_repaint();
            })
            .map_err(|error| format!("Could not start terminal worker: {error}"))?;
        Ok(session)
    }

    fn new(ctx: egui::Context) -> (Self, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::channel();
        let size = PtySize {
            cols: INITIAL_COLUMNS,
            rows: INITIAL_ROWS,
            pixel_width: 0,
            pixel_height: 0,
        };
        let state = Arc::new(Mutex::new(State {
            title: "PowerShell".into(),
            status: "Starting…".into(),
            running: true,
            size,
        }));
        let proxy = Proxy {
            commands: commands.clone(),
            state: state.clone(),
            ctx: ctx.clone(),
        };
        let term = Term::new(
            terminal_config(size.cols),
            &TermSize::new(size.cols.into(), size.rows.into()),
            proxy,
        );
        (
            Self {
                term: Arc::new(Mutex::new(term)),
                commands,
                state,
                ctx,
                size,
                preedit: String::new(),
                scroll_rows: 0.0,
                #[cfg(test)]
                test_commands: None,
                #[cfg(test)]
                test_processor: Processor::new(),
            },
            receiver,
        )
    }

    pub(crate) fn title(&self) -> String {
        self.state.lock().unwrap().title.clone()
    }

    pub(crate) fn status(&self) -> String {
        self.state.lock().unwrap().status.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    pub(crate) fn stop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }

    fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() && self.is_running() {
            let _ = self.commands.send(Command::Write(bytes));
        }
    }

    fn resize(&mut self, size: PtySize) {
        if self.size == size {
            return;
        }
        self.size = size;
        self.state.lock().unwrap().size = size;
        let mut term = self.term.lock().unwrap();
        term.set_options(terminal_config(size.cols));
        term.resize(TermSize::new(size.cols.into(), size.rows.into()));
        drop(term);
        let _ = self.commands.send(Command::Resize(size));
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        let (mut session, receiver) = Self::new(egui::Context::default());
        session.state.lock().unwrap().status = "Running".into();
        session.test_commands = Some(receiver);
        session
    }

    #[cfg(test)]
    pub(crate) fn feed_output(&mut self, bytes: &[u8]) {
        self.test_processor
            .advance(&mut *self.term.lock().unwrap(), bytes);
    }

    #[cfg(test)]
    pub(crate) fn take_input_bytes(&mut self) -> Vec<u8> {
        self.test_commands
            .as_ref()
            .into_iter()
            .flat_map(|receiver| receiver.try_iter())
            .filter_map(|command| match command {
                Command::Write(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.stop();
    }
}

fn terminal_config(columns: u16) -> Config {
    Config {
        scrolling_history: MAX_HISTORY_LINES.min(MAX_HISTORY_CELLS / usize::from(columns.max(2))),
        // We provide the conventional xterm keyboard protocol. Do not advertise
        // the optional kitty protocol until its complete encoder is available.
        kitty_keyboard: false,
        ..Config::default()
    }
}

#[cfg(test)]
mod tests;
