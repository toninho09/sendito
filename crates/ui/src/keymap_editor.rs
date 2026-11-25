//! The key bindings editor (`:keymap`): every action, grouped as the help
//! groups them, with its keys. Enter records the next keys pressed as the
//! action's keys; a pause of one second ends the recording. Ctrl+S writes
//! `[keymap.<mode>]` to the config file the settings choose.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sendito_store::config::{self, KeySpec};

use crate::app::{App, StatusKind};
use crate::editor::TextEditor;
use crate::keymap::{Action, Chord, Ctx, KeySeq, Keymap, Lookup};
use crate::overlay::{Confirm, ConfirmAction, Overlay};

/// The keymap tabs: the `[keymap.<mode>]` names.
pub const KEYMAP_MODES: &[&str] = &["all", "conventional", "vim"];

/// The pause that ends a recording.
pub const CAPTURE_PAUSE: Duration = Duration::from_secs(1);

/// Keys being recorded for an action.
pub struct Capture {
    pub action: Action,
    pub keys: Vec<Chord>,
    /// The last key press (the pause counts from it).
    pub at: Instant,
    /// The keys go after the action's keys instead of replacing them.
    pub add: bool,
}

/// One row of the list: a group title or an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRow {
    Group(Ctx),
    Action(Action),
}

pub struct KeymapEditor {
    /// The config file Ctrl+S writes.
    pub path: PathBuf,
    /// The selected tab, an index into `KEYMAP_MODES`.
    pub mode: usize,
    /// The file's keys per tab: action → keys as written (empty = unbound).
    pub overrides: [BTreeMap<Action, Vec<String>>; 3],
    /// Entries of the file this editor does not know (kept as they are).
    pub unknown: BTreeMap<String, BTreeMap<String, KeySpec>>,
    pub filter: TextEditor,
    /// The selected action, as an index into the actions of `rows()`.
    pub sel: usize,
    pub scroll: usize,
    pub capture: Option<Capture>,
    pub dirty: bool,
    pub error: Option<String>,
    /// A notice about the last change (e.g. a key used twice).
    pub note: Option<String>,
}

/// The context an action's keys belong to (its first default binding).
fn ctx_of(defaults: &Keymap, a: Action) -> Ctx {
    defaults
        .bindings
        .iter()
        .find(|b| b.action == a)
        .map(|b| b.ctx)
        .unwrap_or(Ctx::Global)
}

impl KeymapEditor {
    pub fn new(path: PathBuf, file: &BTreeMap<String, BTreeMap<String, KeySpec>>) -> Self {
        let mut overrides: [BTreeMap<Action, Vec<String>>; 3] = Default::default();
        let mut unknown: BTreeMap<String, BTreeMap<String, KeySpec>> = BTreeMap::new();
        for (mode, actions) in file {
            // `default` is an old name of `all`.
            let tab = KEYMAP_MODES
                .iter()
                .position(|m| m == mode || (*m == "all" && mode == "default"));
            for (id, spec) in actions {
                match (tab, Action::from_id(id)) {
                    (Some(t), Some(a)) => {
                        overrides[t].insert(a, spec.keys());
                    }
                    _ => {
                        unknown
                            .entry(mode.clone())
                            .or_default()
                            .insert(id.clone(), spec.clone());
                    }
                }
            }
        }
        Self {
            path,
            mode: 0,
            overrides,
            unknown,
            filter: TextEditor::single("").with_placeholder("type to filter actions"),
            sel: 0,
            scroll: 0,
            capture: None,
            dirty: false,
            error: None,
            note: None,
        }
    }

    /// The editor's keys as `[keymap]`: the tabs with entries, plus the
    /// entries it does not know.
    pub fn to_config(&self) -> BTreeMap<String, BTreeMap<String, KeySpec>> {
        let mut out = self.unknown.clone();
        for (t, map) in self.overrides.iter().enumerate() {
            for (a, keys) in map {
                let spec = match keys.as_slice() {
                    [one] => KeySpec::One(one.clone()),
                    _ => KeySpec::Many(keys.clone()),
                };
                out.entry(KEYMAP_MODES[t].to_string())
                    .or_default()
                    .insert(a.id().to_string(), spec);
            }
        }
        out
    }

    /// The keymap the editor's keys give.
    pub fn keymap(&self) -> Keymap {
        Keymap::defaults().with_overrides(&self.to_config()).0
    }

    /// The keys an action has in the selected tab, and whether the file
    /// sets them (in this tab).
    pub fn keys(&self, a: Action) -> (Vec<String>, bool) {
        match self.overrides[self.mode].get(&a) {
            Some(keys) => (keys.clone(), true),
            None => (self.keymap().keys_for(a, self.mode == 2), false),
        }
    }

    /// The rows the list shows: groups, then their actions that match the
    /// filter (the description, the id or a key).
    pub fn rows(&self) -> Vec<KeyRow> {
        let defaults = Keymap::defaults();
        let km = self.keymap();
        let f = self.filter.text().to_lowercase();
        let matches = |a: Action| {
            f.is_empty()
                || a.description().to_lowercase().contains(&f)
                || a.id().contains(&f)
                || km
                    .keys_for(a, self.mode == 2)
                    .iter()
                    .any(|k| k.to_lowercase().contains(&f))
        };
        let mut out = Vec::new();
        for ctx in [
            Ctx::Global,
            Ctx::Tree,
            Ctx::Editor,
            Ctx::Table,
            Ctx::Response,
        ] {
            let actions: Vec<Action> = Action::ALL
                .iter()
                .copied()
                .filter(|a| ctx_of(&defaults, *a) == ctx && matches(*a))
                .collect();
            if !actions.is_empty() {
                out.push(KeyRow::Group(ctx));
                out.extend(actions.into_iter().map(KeyRow::Action));
            }
        }
        out
    }

    /// The selected action.
    pub fn selected(&self) -> Option<Action> {
        self.rows()
            .into_iter()
            .filter_map(|r| match r {
                KeyRow::Action(a) => Some(a),
                KeyRow::Group(_) => None,
            })
            .nth(self.sel)
    }

    fn actions_len(&self) -> usize {
        self.rows()
            .iter()
            .filter(|r| matches!(r, KeyRow::Action(_)))
            .count()
    }

    fn step(&mut self, d: isize) {
        self.sel = crate::list::step(self.sel, d, self.actions_len());
    }

    /// The mouse wheel moves the selection (not while recording).
    pub fn wheel(&mut self, d: isize) {
        if self.capture.is_none() {
            self.step(d);
        }
    }

    /// Sets the keys of `a` in the selected tab (`None` = back to the
    /// default) and says when a key now does two things.
    fn set_keys(&mut self, a: Action, keys: Option<Vec<String>>) {
        match keys {
            Some(k) => {
                self.overrides[self.mode].insert(a, k);
            }
            None => {
                self.overrides[self.mode].remove(&a);
            }
        }
        self.dirty = true;
        self.error = None;
        self.note = self.conflict(a);
    }

    /// A key of `a` that another action of the same context also uses, or
    /// that starts (or is the start of) another key there.
    pub fn conflict(&self, a: Action) -> Option<String> {
        let km = self.keymap();
        let defaults = Keymap::defaults();
        let ctx = ctx_of(&defaults, a);
        let vim = self.mode == 2;
        for key in km.keys_for(a, vim) {
            let Ok(seq) = KeySeq::parse(&key) else {
                continue;
            };
            for other in Action::ALL.iter().copied().filter(|o| *o != a) {
                if ctx_of(&defaults, other) != ctx && ctx_of(&defaults, other) != Ctx::Global {
                    continue;
                }
                for k in km.keys_for(other, vim) {
                    let Ok(o) = KeySeq::parse(&k) else {
                        continue;
                    };
                    let n = o.0.len().min(seq.0.len());
                    if o.0[..n] == seq.0[..n] {
                        return Some(format!(
                            "⚠ {key} clashes with {k} of \"{}\": only one of them works",
                            other.description()
                        ));
                    }
                }
            }
        }
        // A key that only starts a longer one, in the action's own context.
        if let Some(key) = km.keys_for(a, vim).first()
            && let Ok(seq) = KeySeq::parse(key)
            && km.lookup(&[ctx], vim, &seq.0) == Lookup::Prefix
        {
            return Some(format!("⚠ {key} starts a longer key: it never runs alone"));
        }
        None
    }

    /// Ends a recording: the keys become the action's keys (or are added).
    fn commit(&mut self) {
        let Some(c) = self.capture.take() else {
            return;
        };
        if c.keys.is_empty() {
            return;
        }
        let seq = KeySeq(c.keys).to_string();
        let mut keys = if c.add {
            self.keys(c.action).0
        } else {
            Vec::new()
        };
        if !keys.contains(&seq) {
            keys.push(seq);
        }
        self.set_keys(c.action, Some(keys));
    }

    /// Ends a recording that has paused long enough. Returns whether it did.
    pub fn capture_due(&mut self) -> bool {
        let due = self
            .capture
            .as_ref()
            .is_some_and(|c| !c.keys.is_empty() && c.at.elapsed() >= CAPTURE_PAUSE);
        if due {
            self.commit();
        }
        due
    }

    /// Starts recording the keys of the selected action.
    fn start_capture(&mut self, add: bool) {
        if let Some(action) = self.selected() {
            self.capture = Some(Capture {
                action,
                keys: Vec::new(),
                at: Instant::now(),
                add,
            });
            self.note = None;
            self.error = None;
        }
    }
}

/// The editor's buttons, in order.
pub const BUTTONS: &[&str] = &[
    "Save (Ctrl+S)",
    "Change (Enter)",
    "Default (Del)",
    "Close (Esc)",
];

impl App {
    /// `:keymap`: the key bindings of the settings scope's file.
    pub fn open_keymap_editor(&mut self) {
        self.clear_transient_status();
        let Some(path) = self.scope_file() else {
            return;
        };
        let Some(cfg) = self.load_one(&path) else {
            return;
        };
        self.overlay = Some(Overlay::Keymap(KeymapEditor::new(path, &cfg.keymap)));
    }

    /// Writes the editor's keys to its file and applies them. Returns false
    /// (with the reason in the editor) when the write failed.
    pub fn save_keymap(&mut self, ed: &mut KeymapEditor) -> bool {
        if let Err(e) = config::set_table(&ed.path, &["keymap"], &ed.to_config()) {
            ed.error = Some(e.to_string());
            return false;
        }
        self.project.config.keymap = self.merged_config().keymap;
        let (keymap, warnings) = Keymap::defaults().with_overrides(&self.project.config.keymap);
        self.keymap = keymap;
        if !warnings.is_empty() {
            ed.error = Some(warnings.join("; "));
            return false;
        }
        ed.dirty = false;
        ed.error = None;
        let file = crate::app::display_path(&ed.path, self.home.as_deref());
        self.set_status(format!("saved to {file}"), StatusKind::Ok);
        true
    }

    /// Keys of the keymap editor; false closes it.
    pub(crate) fn keymap_key(&mut self, e: &mut KeymapEditor, key: KeyEvent) -> bool {
        if let Some(c) = &mut e.capture {
            if key.code == KeyCode::Esc {
                e.capture = None;
                e.note = Some("recording cancelled; the keys did not change".into());
            } else {
                c.keys.push(Chord::from_event(&key));
                c.at = Instant::now();
            }
            return true;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if e.dirty {
                    let ed =
                        std::mem::replace(e, KeymapEditor::new(PathBuf::new(), &BTreeMap::new()));
                    self.parked_keymap = Some(ed);
                    self.overlay = Some(Overlay::Confirm(Confirm {
                        title: "Unsaved changes".into(),
                        message: "Save the changes to the config file?".into(),
                        buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                        sel: 0,
                        action: ConfirmAction::KeymapClose,
                    }));
                }
                return false;
            }
            KeyCode::Char('s') if ctrl => {
                self.save_keymap(e);
            }
            KeyCode::Char('a') if ctrl => e.start_capture(true),
            KeyCode::Char('d') if ctrl => {
                if let Some(a) = e.selected() {
                    e.set_keys(a, Some(Vec::new()));
                    e.note
                        .get_or_insert_with(|| format!("{} has no keys now", a.id()));
                }
            }
            KeyCode::Enter => e.start_capture(false),
            KeyCode::Delete => {
                if let Some(a) = e.selected() {
                    e.set_keys(a, None);
                    e.note
                        .get_or_insert_with(|| format!("{} has its default keys", a.id()));
                }
            }
            KeyCode::Up => e.step(-1),
            KeyCode::Down => e.step(1),
            KeyCode::PageUp => e.step(-10),
            KeyCode::PageDown => e.step(10),
            KeyCode::Tab | KeyCode::BackTab => {
                let d = if key.code == KeyCode::BackTab
                    || key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    -1
                } else {
                    1
                };
                e.mode = crate::list::cycle(e.mode, d, KEYMAP_MODES.len());
                e.note = None;
            }
            _ => {
                if e.filter.handle_key(&key, false, &mut self.clipboard)
                    == crate::editor::Outcome::Changed
                {
                    e.sel = 0;
                    e.scroll = 0;
                }
            }
        }
        true
    }

    /// The answer to the "unsaved changes" question of the keymap editor.
    pub(crate) fn keymap_close(&mut self, sel: usize) {
        let Some(mut ed) = self.parked_keymap.take() else {
            return;
        };
        match sel {
            0 if self.save_keymap(&mut ed) => {}
            0 | 2.. => self.overlay = Some(Overlay::Keymap(ed)),
            _ => self.set_status("closed without saving", StatusKind::Info),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(toml: &str) -> KeymapEditor {
        let file: BTreeMap<String, BTreeMap<String, KeySpec>> = toml::from_str(toml).unwrap();
        KeymapEditor::new(PathBuf::new(), &file)
    }

    #[test]
    fn reads_and_writes_the_file_keys() {
        let e = editor(
            "[default]\nsend = \"f9\"\n[vim]\ntree_delete = [\"D\", \"x\"]\n\
             [all]\nnot_an_action = \"f2\"\n",
        );
        assert_eq!(e.overrides[0][&Action::Send], vec!["f9"]);
        assert_eq!(e.overrides[2][&Action::TreeDelete], vec!["D", "x"]);
        let out = e.to_config();
        assert_eq!(out["all"]["send"], KeySpec::One("f9".into()));
        // An entry the editor does not know is kept as it is.
        assert_eq!(out["all"]["not_an_action"], KeySpec::One("f2".into()));
    }

    #[test]
    fn records_keys_and_reports_clashes() {
        let mut e = editor("");
        e.filter.set_text("pretty-print");
        assert_eq!(e.selected(), Some(Action::FormatBody));
        e.start_capture(false);
        for c in ["ctrl+w", "f"] {
            if let Some(cap) = &mut e.capture {
                cap.keys.push(Chord::parse(c).unwrap());
            }
        }
        // Not yet: the pause has not passed.
        assert!(!e.capture_due());
        if let Some(cap) = &mut e.capture {
            cap.at -= CAPTURE_PAUSE;
        }
        assert!(e.capture_due());
        assert_eq!(e.overrides[0][&Action::FormatBody], vec!["Ctrl+W f"]);
        assert!(e.note.is_none(), "{:?}", e.note);
        // Ctrl+S is the key of Save: the editor says so.
        e.set_keys(Action::FormatBody, Some(vec!["ctrl+s".into()]));
        assert!(e.note.as_deref().unwrap().contains("Save"), "{:?}", e.note);
        // Ctrl+W alone starts every menu key.
        e.set_keys(Action::FormatBody, Some(vec!["ctrl+w".into()]));
        assert!(e.note.is_some());
        // Back to the default.
        e.set_keys(Action::FormatBody, None);
        assert!(!e.overrides[0].contains_key(&Action::FormatBody));
        assert_eq!(e.keys(Action::FormatBody).0, vec!["Ctrl+W f"]);
    }

    #[test]
    fn every_action_is_listed_once() {
        let e = editor("");
        let rows = e.rows();
        for a in Action::ALL {
            let n = rows.iter().filter(|r| **r == KeyRow::Action(*a)).count();
            assert_eq!(n, 1, "{a:?}");
        }
    }
}
