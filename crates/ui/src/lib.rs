//! Sendito terminal UI.

pub mod app;
pub mod complete;
pub mod draw;
pub mod editor;
pub mod filter;
pub mod form;
pub mod icons;
pub mod keymap;
pub mod list;
pub mod overlay;
pub mod runner;
pub mod tab;
pub mod theme;
pub mod tree;
pub mod viewer;

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use app::App;

/// Placeholder shown instead of secret values.
pub const MASK: &str = sendito_core::vars::MASK;

/// Options for an interactive session.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub root: PathBuf,
    /// Exit right after the first frame is drawn (startup benchmarks).
    pub exit_after_first_frame: bool,
    /// Append one frame time (µs) per line to this file.
    pub frame_log: Option<PathBuf>,
    /// Replay this event script instead of waiting for input (benchmarks).
    pub script: Option<PathBuf>,
    /// Process start, for the startup measurement printed with `exit_after_first_frame`.
    pub started: Option<Instant>,
}

struct TermGuard {
    mouse: bool,
}

impl TermGuard {
    fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
        if mouse {
            execute!(out, EnableMouseCapture)?;
        }
        // Keep the window title to restore it on exit (xterm title stack).
        let _ = out.write_all(b"\x1b[22;0t");
        let _ = out.flush();
        // Terminals without the kitty keyboard protocol ignore this.
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        Ok(Self { mouse })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        if self.mouse {
            let _ = execute!(out, DisableMouseCapture);
        }
        let _ = execute!(
            out,
            crossterm::cursor::SetCursorStyle::DefaultUserShape,
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = out.write_all(b"\x1b[23;0t");
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

fn write_osc52(text: &str) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

/// Runs the interactive UI until the user quits.
pub fn run(
    project: runner::Project,
    opts: RunOptions,
    rt: &tokio::runtime::Runtime,
) -> io::Result<()> {
    let session =
        sendito_store::session::load(&sendito_store::session::session_path(&project.root));
    let mouse = project.config.ui.mouse;
    let mut app = App::new(project, session, rt.handle().clone());
    let script = match &opts.script {
        Some(p) => Some(bench::Script::load(p)?),
        None => None,
    };
    let guard = TermGuard::enter(mouse)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // The alternate screen starts empty; ratatui's `clear()` would also query the
    // cursor position (a terminal round trip) on every start.
    execute!(
        io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    let mut log = match &opts.frame_log {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let result = event_loop(&mut app, &mut terminal, &opts, script, &mut log);
    app.save_session();
    drop(terminal);
    drop(guard);
    if let Some(l) = &mut log {
        let _ = l.flush();
    }
    result
}

fn event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    opts: &RunOptions,
    mut script: Option<bench::Script>,
    log: &mut Option<std::io::BufWriter<std::fs::File>>,
) -> io::Result<()> {
    let mut dirty = true;
    let mut last_anim = Instant::now();
    let mut shape: Option<bool> = None;
    let mut title = String::new();
    loop {
        if dirty {
            let t0 = Instant::now();
            terminal.draw(|f| draw::draw(app, f))?;
            // A bar cursor while typing; a block in vim normal mode.
            if app.last_cursor.is_some() {
                let bar = !app.vim
                    || matches!(app.focused_vim_mode(), Some(editor::VimMode::Insert) | None);
                if shape != Some(bar) {
                    let st = if bar {
                        crossterm::cursor::SetCursorStyle::SteadyBar
                    } else {
                        crossterm::cursor::SetCursorStyle::SteadyBlock
                    };
                    let _ = execute!(io::stdout(), st);
                    shape = Some(bar);
                }
            }
            let t = app.window_title();
            if t != title {
                let _ = execute!(io::stdout(), crossterm::terminal::SetTitle(&t));
                title = t;
            }
            if let Some(l) = log {
                let _ = writeln!(l, "frame {}", t0.elapsed().as_micros());
            }
            for text in app.osc52.drain(..) {
                write_osc52(&text);
            }
            if let Some(t) = app.clipboard.pending.take() {
                write_osc52(&t);
            }
            dirty = false;
            if opts.exit_after_first_frame {
                if let Some(t) = opts.started {
                    eprintln!("first frame after {} µs", t.elapsed().as_micros());
                }
                return Ok(());
            }
        }
        if app.quit {
            return Ok(());
        }
        let busy = app.is_busy();
        let timeout = if busy {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(250)
        };
        if let Some(s) = &mut script {
            match s.next_event() {
                Some(bench::Step::Event(ev)) => {
                    let t = Instant::now();
                    app.on_event(ev);
                    if let Some(l) = log {
                        let _ = writeln!(l, "event {}", t.elapsed().as_micros());
                    }
                    dirty = true;
                }
                Some(bench::Step::Wait(d)) => std::thread::sleep(d),
                None => return Ok(()),
            }
        } else if crossterm::event::poll(timeout)? {
            app.on_event(crossterm::event::read()?);
            // Coalesce bursts (fast typing, wheel, paste) into one frame.
            let burst_start = Instant::now();
            while burst_start.elapsed() < Duration::from_millis(8)
                && crossterm::event::poll(Duration::ZERO)?
            {
                app.on_event(crossterm::event::read()?);
            }
            dirty = true;
        }
        while let Ok(msg) = app.rx.try_recv() {
            let t = Instant::now();
            app.on_msg(msg);
            if let Some(l) = log {
                let _ = writeln!(l, "msg {}", t.elapsed().as_micros());
            }
            dirty = true;
        }
        // Elapsed-time counters and spinners while a request runs.
        if busy && last_anim.elapsed() > Duration::from_millis(100) {
            last_anim = Instant::now();
            app.anim_frame = app.anim_frame.wrapping_add(1);
            dirty = true;
        }
        let before = app.status.is_some();
        if app.tick() || before != app.status.is_some() {
            dirty = true;
        }
    }
}

/// Scripted input for benchmarks and recordings.
pub mod bench {
    use std::time::Duration;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

    pub enum Step {
        Event(Event),
        Wait(Duration),
    }

    /// One step per line: `key <spec>`, `type <text>`, `wait <ms>`, `scroll <up|down> <x> <y>`,
    /// `repeat <n> <step>`.
    pub struct Script {
        steps: Vec<String>,
        pos: usize,
        queue: std::collections::VecDeque<Step>,
    }

    impl Script {
        pub fn load(p: &std::path::Path) -> std::io::Result<Self> {
            Ok(Self::parse(&std::fs::read_to_string(p)?))
        }

        pub fn parse(text: &str) -> Self {
            let mut steps = Vec::new();
            for line in text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                if let Some(rest) = line.strip_prefix("repeat ")
                    && let Some((n, step)) = rest.split_once(' ')
                {
                    for _ in 0..n.parse::<usize>().unwrap_or(1) {
                        steps.push(step.to_string());
                    }
                    continue;
                }
                steps.push(line.to_string());
            }
            Self {
                steps,
                pos: 0,
                queue: Default::default(),
            }
        }

        pub fn next_event(&mut self) -> Option<Step> {
            loop {
                if let Some(s) = self.queue.pop_front() {
                    return Some(s);
                }
                let line = self.steps.get(self.pos)?.clone();
                self.pos += 1;
                let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                match cmd {
                    "wait" => self
                        .queue
                        .push_back(Step::Wait(Duration::from_millis(arg.parse().unwrap_or(0)))),
                    "type" => {
                        for c in arg.chars() {
                            self.queue.push_back(Step::Event(Event::Key(KeyEvent::new(
                                KeyCode::Char(c),
                                KeyModifiers::NONE,
                            ))));
                        }
                    }
                    "key" => {
                        if let Ok(c) = crate::keymap::Chord::parse(arg) {
                            self.queue
                                .push_back(Step::Event(Event::Key(KeyEvent::new(c.code, c.mods))));
                        }
                    }
                    "scroll" => {
                        let parts: Vec<&str> = arg.split_whitespace().collect();
                        let kind = if parts.first() == Some(&"up") {
                            MouseEventKind::ScrollUp
                        } else {
                            MouseEventKind::ScrollDown
                        };
                        let x = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                        let y = parts.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
                        self.queue.push_back(Step::Event(Event::Mouse(MouseEvent {
                            kind,
                            column: x,
                            row: y,
                            modifiers: KeyModifiers::NONE,
                        })));
                    }
                    _ => {}
                }
            }
        }
    }
}
