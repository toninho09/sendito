//! Test harness: a temporary collection, an App driven by synthetic events,
//! and a TestBackend to capture frames.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use sendito_store::env::MemoryKeychain;
use sendito_ui::App;
use sendito_ui::runner::Project;

/// `unwrap` for test helpers (the workspace denies `unwrap_used`).
pub trait Must<T> {
    fn must(self) -> T;
}

impl<T, E: std::fmt::Debug> Must<T> for Result<T, E> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{e:?}"),
        }
    }
}

impl<T> Must<T> for Option<T> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("unexpected None"),
        }
    }
}

pub mod mqtt;
pub mod sse;

pub struct Harness {
    pub app: App,
    pub term: Terminal<TestBackend>,
    pub dir: tempfile::TempDir,
    pub rt: tokio::runtime::Runtime,
}

pub fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().must()).must();
    std::fs::write(p, text).must();
}

/// A small demo collection.
pub fn demo_collection(dir: &Path, base: &str, grpc: &str) {
    sendito_store::init::init(dir).must();
    write(
        dir,
        ".sendito/config.toml",
        "[collection]\nname = \"demo\"\n",
    );
    write(
        dir,
        ".sendito/environments/local.toml",
        &format!(
            "[variables]\nbase_url = \"{base}\"\ngrpc = \"{grpc}\"\ntoken = {{ source = \"local\" }}\n"
        ),
    );
    write(
        dir,
        ".sendito/environments/staging.toml",
        "[variables]\nbase_url = \"https://staging.example.com\"\n",
    );
    write(
        dir,
        ".sendito/secrets.toml",
        "[local]\ntoken = \"s3cr3t-token\"\n",
    );
    write(
        dir,
        "Users/list users.toml",
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base_url}}/users\"\n\n[[params]]\nname = \"page\"\nvalue = \"1\"\n\n[[headers]]\nname = \"Accept\"\nvalue = \"application/json\"\n\n[auth]\ntype = \"bearer\"\ntoken = \"{{token}}\"\n",
    );
    write(
        dir,
        "Users/create user.toml",
        "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base_url}}/users\"\n\n[body]\ntype = \"json\"\ntext = '''\n{\n  \"name\": \"Ana\",\n  \"email\": \"ana@example.com\"\n}\n'''\n\n[[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"200\"\n",
    );
    write(
        dir,
        "Users/delete user.toml",
        "kind = \"http\"\nmethod = \"DELETE\"\nurl = \"{{base_url}}/users/42\"\n",
    );
    write(
        dir,
        "Orders/get order.toml",
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base_url}}/orders/{{order_id}}\"\n",
    );
    write(
        dir,
        "echo unary.toml",
        "kind = \"grpc\"\ntarget = \"{{grpc}}\"\nservice = \"sendito.test.v1.Echo\"\nmethod = \"Unary\"\nmessage = '''\n{\n  \"text\": \"hello\"\n}\n'''\n",
    );
    write(
        dir,
        "health.toml",
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base_url}}/status/200\"\n",
    );
    let _ = grpc;
}

impl Harness {
    pub fn new(w: u16, h: u16) -> Self {
        Self::with(w, h, |d| {
            demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9")
        })
    }

    pub fn with(w: u16, h: u16, setup: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().must();
        setup(dir.path());
        // Isolate from the user's global config: the temp dir plays $HOME.
        let cfg = dir.path().join(".config/sendito");
        std::fs::create_dir_all(&cfg).must();
        // SAFETY-free: tests set XDG_CONFIG_HOME through the project only.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .must();
        let project = Project::open_with(
            dir.path(),
            Arc::new(MemoryKeychain::default()),
            Some(cfg.join("config.toml")),
        );
        let (session, _) =
            sendito_store::session::load(&sendito_store::session::session_path(dir.path()));
        let mut app = App::new(project, session, rt.handle().clone());
        app.home = Some(dir.path().to_path_buf());
        let term = Terminal::new(TestBackend::new(w, h)).must();
        let mut h = Harness { app, term, dir, rt };
        h.draw();
        h
    }

    /// Saves the session and starts the app again on the same collection,
    /// as a restart does.
    #[allow(dead_code)]
    pub fn reopen(&mut self) {
        self.app.save_session();
        let cfg = self.dir.path().join(".config/sendito");
        let project = Project::open_with(
            self.dir.path(),
            Arc::new(MemoryKeychain::default()),
            Some(cfg.join("config.toml")),
        );
        let (session, _) =
            sendito_store::session::load(&sendito_store::session::session_path(self.dir.path()));
        self.app = App::new(project, session, self.rt.handle().clone());
        self.app.home = Some(self.dir.path().to_path_buf());
        self.draw();
    }

    pub fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| sendito_ui::draw::draw(app, f)).must();
    }

    pub fn screen(&mut self) -> String {
        self.draw();
        let buf = self.term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    pub fn key(&mut self, spec: &str) {
        for part in spec.split_whitespace() {
            let c = sendito_ui::keymap::Chord::parse(part).must();
            self.app.on_event(Event::Key(KeyEvent::new(c.code, c.mods)));
            self.pump();
        }
    }

    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.app.on_event(Event::Key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )));
        }
        self.pump();
    }

    pub fn paste(&mut self, s: &str) {
        self.app.on_event(Event::Paste(s.to_string()));
        self.pump();
    }

    pub fn click(&mut self, x: u16, y: u16) {
        self.draw();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            self.app.on_event(Event::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            }));
        }
        self.pump();
    }

    pub fn double_click(&mut self, x: u16, y: u16) {
        self.click(x, y);
        self.click(x, y);
    }

    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.draw();
        let ev = |kind, (x, y): (u16, u16)| {
            Event::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        self.app
            .on_event(ev(MouseEventKind::Down(MouseButton::Left), from));
        self.draw();
        self.app.on_event(ev(
            MouseEventKind::Drag(MouseButton::Left),
            ((from.0 + to.0) / 2, (from.1 + to.1) / 2),
        ));
        self.draw();
        self.app
            .on_event(ev(MouseEventKind::Drag(MouseButton::Left), to));
        self.draw();
        self.app
            .on_event(ev(MouseEventKind::Up(MouseButton::Left), to));
        self.pump();
    }

    pub fn scroll(&mut self, x: u16, y: u16, down: bool) {
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        self.app.on_event(Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }));
        self.pump();
    }

    /// Processes pending async messages.
    pub fn pump(&mut self) {
        while let Ok(m) = self.app.rx.try_recv() {
            self.app.on_msg(m);
        }
        self.draw();
    }

    /// Waits until no request is running (or the timeout passes).
    pub fn wait_idle(&mut self, secs: u64) {
        let start = std::time::Instant::now();
        loop {
            self.pump();
            if !self.app.is_busy() && start.elapsed() > Duration::from_millis(30) {
                // Let late messages arrive.
                std::thread::sleep(Duration::from_millis(20));
                self.pump();
                if !self.app.is_busy() {
                    return;
                }
            }
            if start.elapsed() > Duration::from_secs(secs) {
                panic!("timed out waiting for idle");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_for(&mut self, needle: &str, secs: u64) -> String {
        let start = std::time::Instant::now();
        loop {
            let s = self.screen();
            if s.contains(needle) {
                return s;
            }
            if start.elapsed() > Duration::from_secs(secs) {
                panic!("timed out waiting for {needle:?}; screen:\n{s}");
            }
            std::thread::sleep(Duration::from_millis(10));
            self.pump();
        }
    }

    /// Finds the first screen position of `needle`.
    pub fn find(&mut self, needle: &str) -> Option<(u16, u16)> {
        let s = self.screen();
        for (y, line) in s.lines().enumerate() {
            if let Some(bx) = line.find(needle) {
                let x = line[..bx].chars().count();
                return Some((x as u16, y as u16));
            }
        }
        None
    }
}

/// Styled screen dump for the media renderer (docs/media/render.py).
pub fn buffer_json(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::{Color, Modifier};
    fn color(c: Color) -> String {
        match c {
            Color::Reset => "reset".into(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Indexed(i) => format!("i{i}"),
            other => format!("{other:?}").to_lowercase(),
        }
    }
    let mut rows = Vec::new();
    for y in 0..buf.area.height {
        let mut cells = Vec::new();
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let m = c.modifier;
            let mut flags = String::new();
            if m.contains(Modifier::BOLD) {
                flags.push('b');
            }
            if m.contains(Modifier::ITALIC) {
                flags.push('i');
            }
            if m.contains(Modifier::UNDERLINED) {
                flags.push('u');
            }
            if m.contains(Modifier::REVERSED) {
                flags.push('r');
            }
            if m.contains(Modifier::DIM) {
                flags.push('d');
            }
            if m.contains(Modifier::CROSSED_OUT) {
                flags.push('x');
            }
            cells.push(serde_json::json!([
                c.symbol(),
                color(c.fg),
                color(c.bg),
                flags
            ]));
        }
        rows.push(serde_json::Value::Array(cells));
    }
    serde_json::json!({"width": buf.area.width, "height": buf.area.height, "rows": rows})
        .to_string()
}
