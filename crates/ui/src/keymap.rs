//! Actions and key bindings. This table is the single source for key
//! handling, the in-app help (F1) and KEYMAP.md.

use std::collections::BTreeMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sendito_store::config::KeySpec;

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ctx {
    Global,
    Tree,
    Editor,
    Table,
    Response,
}

impl Ctx {
    pub fn title(self) -> &'static str {
        match self {
            Ctx::Global => "Global",
            Ctx::Tree => "Sidebar tree",
            Ctx::Editor => "Text editing (conventional mode)",
            Ctx::Table => "Params / headers / form tables",
            Ctx::Response => "Response viewer",
        }
    }
}

/// Which editing mode a binding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeSet {
    Both,
    Conventional,
    Vim,
}

macro_rules! actions {
    ($( $v:ident => $id:literal, $desc:literal; )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action { $($v),* }
        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$v),*];
            pub fn id(self) -> &'static str { match self { $(Action::$v => $id),* } }
            pub fn description(self) -> &'static str { match self { $(Action::$v => $desc),* } }
            pub fn from_id(s: &str) -> Option<Action> { Self::ALL.iter().copied().find(|a| a.id() == s) }
        }
    };
}

actions! {
    Send => "send", "Send the request (or start the gRPC call)";
    Cancel => "cancel", "Cancel the running request or stream";
    EndStream => "end_stream", "Finish sending on a client/bidi stream (half-close)";
    WatchToggle => "watch", "Re-send the active request on an interval (watch)";
    Save => "save", "Save the request to its file (asks for a name if new)";
    Quit => "quit", "Quit Sendito";
    NewRequest => "new_request", "Open a new unsaved HTTP request tab";
    NewGrpcRequest => "new_grpc_request", "Open a new unsaved gRPC request tab";
    NewSseRequest => "new_sse_request", "Open a new unsaved SSE request tab";
    NewMqttRequest => "new_mqtt_request", "Open a new unsaved MQTT request tab";
    NewWsRequest => "new_ws_request", "Open a new unsaved WebSocket request tab";
    CloseTab => "close_tab", "Close the current tab";
    NextTab => "next_tab", "Next open request tab";
    PrevTab => "prev_tab", "Previous open request tab";
    TogglePin => "toggle_pin", "Pin or unpin the current tab (pinned tabs stay first)";
    FocusLeft => "focus_left", "Focus the pane to the left";
    FocusDown => "focus_down", "Focus the pane below";
    FocusUp => "focus_up", "Focus the pane above";
    FocusRight => "focus_right", "Focus the pane to the right";
    FocusNext => "focus_next", "Move focus to the next field or pane";
    FocusPrev => "focus_prev", "Move focus to the previous field or pane";
    NextSubTab => "next_subtab", "Next tab of the focused pane (e.g. Params → Headers)";
    PrevSubTab => "prev_subtab", "Previous tab of the focused pane";
    SubTab1 => "subtab_1", "Jump to tab 1 of the focused pane";
    SubTab2 => "subtab_2", "Jump to tab 2 of the focused pane";
    SubTab3 => "subtab_3", "Jump to tab 3 of the focused pane";
    SubTab4 => "subtab_4", "Jump to tab 4 of the focused pane";
    SubTab5 => "subtab_5", "Jump to tab 5 of the focused pane";
    SubTab6 => "subtab_6", "Jump to tab 6 of the focused pane";
    Palette => "palette", "Open the command palette";
    Help => "help", "Show all keys and actions";
    Settings => "settings", "Open settings";
    SelectEnv => "select_env", "Choose the active environment";
    ManageEnvs => "manage_envs", "Edit environments and their variables";
    ManageMcpServers => "manage_mcp_servers", "Edit the MCP servers in the config file chosen in Settings";
    History => "history", "Show request history in the sidebar (toggle)";
    ToggleSidebar => "toggle_sidebar", "Show or hide the sidebar";
    Import => "import", "Import a collection or a curl command";
    ExportCurl => "export_curl", "Copy the request as curl (secrets stay {{placeholders}})";
    FormatBody => "format_body", "Pretty-print the JSON body / message";
    MoveToSecret => "move_to_secret", "Move the plain value of the focused credential field into an environment secret";
    RevealSecrets => "reveal_secrets", "Show or hide credentials typed as plain text (with Hide plain secrets on)";
    Zoom => "zoom", "Maximize or restore the focused pane";
    GrowSidebar => "grow_sidebar", "Widen the sidebar";
    ShrinkSidebar => "shrink_sidebar", "Narrow the sidebar";
    GrowRequest => "grow_request", "Give the request editor more height";
    ShrinkRequest => "shrink_request", "Give the response more height";
    TreeOpen => "tree_open", "Open the selected request / toggle the folder";
    TreeExpand => "tree_expand", "Expand folder";
    TreeCollapse => "tree_collapse", "Collapse folder or go to parent";
    TreeNewRequest => "tree_new_request", "New request in the selected folder";
    TreeNewFolder => "tree_new_folder", "New folder in the selected folder";
    TreeRename => "tree_rename", "Rename the selected item";
    TreeFolderSettings => "tree_folder_settings", "Edit the settings of the selected folder (headers, auth, variables and scripts its requests inherit)";
    TreeDuplicate => "tree_duplicate", "Duplicate the selected item";
    TreeDelete => "tree_delete", "Delete the selected item (asks first)";
    TreeMove => "tree_move", "Move the selected item to another folder";
    TreeFilter => "tree_filter", "Filter the tree by name";
    ToggleFavorite => "toggle_favorite", "Add or remove the request from the favorites";
    CursorUp => "up", "Move up";
    CursorDown => "down", "Move down";
    PageUp => "page_up", "Page up";
    PageDown => "page_down", "Page down";
    Top => "top", "Go to the first line";
    Bottom => "bottom", "Go to the last line";
    ViewPretty => "view_pretty", "Body view: pretty (formatted and highlighted)";
    ViewRaw => "view_raw", "Body view: raw text";
    ViewHex => "view_hex", "Body view: hex dump";
    Search => "search", "Search in the response";
    Filter => "filter", "Filter the response body with a jq expression";
    FilterHere => "filter_here", "Filter the body to the JSON value at the cursor";
    CopyPath => "copy_path", "Copy the jq path of the JSON value at the cursor";
    PrevMessage => "prev_message", "Show the previous message of a stream";
    NextMessage => "next_message", "Show the next message of a stream (the last one follows new ones)";
    SearchNext => "search_next", "Next search match";
    SearchPrev => "search_prev", "Previous search match";
    FoldToggle => "fold_toggle", "Fold or unfold the block at the cursor";
    FoldAll => "fold_all", "Fold all blocks";
    UnfoldAll => "unfold_all", "Unfold all blocks";
    ToggleWrap => "toggle_wrap", "Wrap long lines on/off";
    CopyBody => "copy_body", "Copy the response body to the clipboard";
    SaveBody => "save_body", "Save the response body to a file";
    ScrollLeft => "scroll_left", "Scroll left";
    ScrollRight => "scroll_right", "Scroll right";
    NextCell => "next_cell", "Edit the next cell (name → value → next row)";
    ToggleRow => "toggle_row", "Enable or disable the row";
    DeleteRow => "delete_row", "Delete the row";
    Undo => "undo", "Undo";
    Redo => "redo", "Redo";
    SelectAll => "select_all", "Select all text";
    Copy => "copy", "Copy selection (whole field if nothing selected)";
    Cut => "cut", "Cut selection";
    Paste => "paste", "Paste";
    Complete => "complete", "Show completion suggestions at the cursor";
}

/// One key press: a key and modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }.normalized()
    }

    /// Canonical form: characters carry their case instead of SHIFT, BackTab is Shift+Tab.
    pub fn normalized(mut self) -> Self {
        match self.code {
            KeyCode::Char(c) => {
                if self.mods.contains(KeyModifiers::SHIFT) {
                    self.mods.remove(KeyModifiers::SHIFT);
                    if c.is_ascii_lowercase() {
                        self.code = KeyCode::Char(c.to_ascii_uppercase());
                    }
                }
                if self.mods.contains(KeyModifiers::CONTROL) && c.is_ascii_uppercase() {
                    self.code = KeyCode::Char(c.to_ascii_lowercase());
                }
            }
            KeyCode::BackTab => {
                self.code = KeyCode::Tab;
                self.mods.insert(KeyModifiers::SHIFT);
            }
            _ => {}
        }
        self.mods &= KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        self
    }

    pub fn from_event(e: &KeyEvent) -> Self {
        Self::new(e.code, e.modifiers)
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".into());
        }
        let parts: Vec<&str> = if s == "+" || s.ends_with("++") {
            let mut p: Vec<&str> = s
                .trim_end_matches('+')
                .split('+')
                .filter(|x| !x.is_empty())
                .collect();
            p.push("+");
            p
        } else {
            s.split('+').collect()
        };
        let mut mods = KeyModifiers::NONE;
        let (key, modifiers) = parts.split_last().ok_or("empty key")?;
        for m in modifiers {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" | "opt" | "option" | "m" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                o => return Err(format!("unknown modifier {o:?}")),
            }
        }
        let lower = key.to_ascii_lowercase();
        let code = match lower.as_str() {
            "enter" | "return" | "cr" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "space" => KeyCode::Char(' '),
            f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                let n: u8 = f[1..].parse().map_err(|_| "bad function key")?;
                if !(1..=12).contains(&n) {
                    return Err(format!("unknown key {key:?}"));
                }
                KeyCode::F(n)
            }
            _ if key.chars().count() == 1 => KeyCode::Char(key.chars().next().unwrap_or(' ')),
            _ => return Err(format!("unknown key {key:?}")),
        };
        Ok(Chord::new(code, mods))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("Ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("Alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            f.write_str("Shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) if self.mods.contains(KeyModifiers::CONTROL) => {
                write!(f, "{}", c.to_ascii_uppercase())
            }
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Insert => f.write_str("Insert"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PageUp"),
            KeyCode::PageDown => f.write_str("PageDown"),
            KeyCode::Up => f.write_str("Up"),
            KeyCode::Down => f.write_str("Down"),
            KeyCode::Left => f.write_str("Left"),
            KeyCode::Right => f.write_str("Right"),
            KeyCode::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// A sequence of chords, e.g. `Ctrl+W h`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySeq(pub Vec<Chord>);

impl KeySeq {
    pub fn parse(s: &str) -> Result<Self, String> {
        let chords: Result<Vec<Chord>, String> = s.split_whitespace().map(Chord::parse).collect();
        let chords = chords?;
        if chords.is_empty() {
            return Err("empty key sequence".into());
        }
        Ok(KeySeq(chords))
    }
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub action: Action,
    pub ctx: Ctx,
    pub modes: ModeSet,
    pub keys: Vec<KeySeq>,
}

/// Default bindings: (action, context, modes, keys).
const DEFAULTS: &[(Action, Ctx, ModeSet, &[&str])] = {
    use Action::*;
    use Ctx::*;
    use ModeSet::*;
    &[
        (Send, Global, Both, &["ctrl+enter", "f5", "ctrl+w e"]),
        (Cancel, Global, Both, &["ctrl+w x"]),
        (EndStream, Global, Both, &["ctrl+w d"]),
        (WatchToggle, Global, Both, &["ctrl+w m"]),
        (Save, Global, Both, &["ctrl+s", "ctrl+w s"]),
        (Quit, Global, Both, &["ctrl+w q", "ctrl+q"]),
        (NewRequest, Global, Both, &["ctrl+n", "ctrl+w t"]),
        (NewGrpcRequest, Global, Both, &["ctrl+w g"]),
        (NewWsRequest, Global, Both, &["ctrl+w y"]),
        (NewSseRequest, Global, Both, &["ctrl+w E"]),
        (NewMqttRequest, Global, Both, &["ctrl+w M"]),
        (CloseTab, Global, Both, &["ctrl+w c"]),
        (NextTab, Global, Both, &["ctrl+w n", "ctrl+pagedown"]),
        (PrevTab, Global, Both, &["ctrl+w p", "ctrl+pageup"]),
        (TogglePin, Global, Both, &["ctrl+w P"]),
        (ToggleFavorite, Global, Both, &["ctrl+w F"]),
        (FocusLeft, Global, Both, &["ctrl+w h", "ctrl+w left"]),
        (FocusDown, Global, Both, &["ctrl+w j", "ctrl+w down"]),
        (FocusUp, Global, Both, &["ctrl+w k", "ctrl+w up"]),
        (FocusRight, Global, Both, &["ctrl+w l", "ctrl+w right"]),
        (FocusNext, Global, Both, &["tab", "ctrl+w w"]),
        (FocusPrev, Global, Both, &["shift+tab", "ctrl+w W"]),
        (NextSubTab, Global, Both, &["ctrl+w ]"]),
        (PrevSubTab, Global, Both, &["ctrl+w ["]),
        (SubTab1, Global, Both, &["alt+1"]),
        (SubTab2, Global, Both, &["alt+2"]),
        (SubTab3, Global, Both, &["alt+3"]),
        (SubTab4, Global, Both, &["alt+4"]),
        (SubTab5, Global, Both, &["alt+5"]),
        (SubTab6, Global, Both, &["alt+6"]),
        (Palette, Global, Both, &["ctrl+p"]),
        (Help, Global, Both, &["f1", "ctrl+w ?"]),
        (Settings, Global, Both, &["ctrl+w ,"]),
        (SelectEnv, Global, Both, &["f4", "ctrl+w v"]),
        (ManageEnvs, Global, Both, &["ctrl+w V"]),
        (ManageMcpServers, Global, Both, &["ctrl+w S"]),
        (History, Global, Both, &["f7", "ctrl+w r"]),
        (ToggleSidebar, Global, Both, &["ctrl+b", "ctrl+w b"]),
        (Import, Global, Both, &["ctrl+w i"]),
        (ExportCurl, Global, Both, &["ctrl+w u"]),
        (FormatBody, Global, Both, &["ctrl+w f"]),
        (MoveToSecret, Global, Both, &["ctrl+w K"]),
        (RevealSecrets, Global, Both, &["ctrl+w *"]),
        (Zoom, Global, Both, &["ctrl+w z"]),
        (GrowSidebar, Global, Both, &["ctrl+w >"]),
        (ShrinkSidebar, Global, Both, &["ctrl+w <"]),
        (GrowRequest, Global, Both, &["ctrl+w +"]),
        (ShrinkRequest, Global, Both, &["ctrl+w -"]),
        (TreeOpen, Tree, Both, &["enter"]),
        (TreeExpand, Tree, Conventional, &["right"]),
        (TreeExpand, Tree, Vim, &["l", "right"]),
        (TreeCollapse, Tree, Conventional, &["left"]),
        (TreeCollapse, Tree, Vim, &["h", "left"]),
        (TreeNewRequest, Tree, Both, &["n"]),
        (TreeNewFolder, Tree, Both, &["N"]),
        (TreeRename, Tree, Both, &["r", "f2"]),
        (TreeFolderSettings, Tree, Both, &["s"]),
        (TreeDuplicate, Tree, Both, &["d"]),
        (TreeDelete, Tree, Both, &["delete", "x"]),
        (TreeMove, Tree, Both, &["m"]),
        (TreeFilter, Tree, Both, &["/"]),
        (ToggleFavorite, Tree, Both, &["f"]),
        (CursorUp, Tree, Conventional, &["up"]),
        (CursorUp, Tree, Vim, &["k", "up"]),
        (CursorDown, Tree, Conventional, &["down"]),
        (CursorDown, Tree, Vim, &["j", "down"]),
        (PageUp, Tree, Both, &["pageup"]),
        (PageDown, Tree, Both, &["pagedown"]),
        (Top, Tree, Conventional, &["home"]),
        (Top, Tree, Vim, &["g g", "home"]),
        (Bottom, Tree, Conventional, &["end"]),
        (Bottom, Tree, Vim, &["G", "end"]),
        (CursorUp, Response, Conventional, &["up"]),
        (CursorUp, Response, Vim, &["k", "up"]),
        (CursorDown, Response, Conventional, &["down"]),
        (CursorDown, Response, Vim, &["j", "down"]),
        (PageUp, Response, Conventional, &["pageup"]),
        (PageUp, Response, Vim, &["ctrl+b", "ctrl+u", "pageup"]),
        (PageDown, Response, Conventional, &["pagedown"]),
        (PageDown, Response, Vim, &["ctrl+f", "ctrl+d", "pagedown"]),
        (Top, Response, Conventional, &["home", "ctrl+home"]),
        (Top, Response, Vim, &["g g", "home"]),
        (Bottom, Response, Conventional, &["end", "ctrl+end"]),
        (Bottom, Response, Vim, &["G", "end"]),
        (ScrollLeft, Response, Conventional, &["left"]),
        (ScrollLeft, Response, Vim, &["h", "left"]),
        (ScrollRight, Response, Conventional, &["right"]),
        (ScrollRight, Response, Vim, &["l", "right"]),
        (ViewPretty, Response, Both, &["p"]),
        (ViewRaw, Response, Both, &["r"]),
        (ViewHex, Response, Both, &["x"]),
        (Search, Response, Both, &["/", "ctrl+f"]),
        (Filter, Response, Conventional, &["j"]),
        (Filter, Response, Vim, &["J"]),
        (FilterHere, Response, Both, &["f"]),
        (CopyPath, Response, Both, &["Y"]),
        (PrevMessage, Response, Both, &["["]),
        (NextMessage, Response, Both, &["]"]),
        (SearchNext, Response, Both, &["n", "f3"]),
        (SearchPrev, Response, Both, &["N", "shift+f3"]),
        (FoldToggle, Response, Conventional, &["enter", "space"]),
        (FoldToggle, Response, Vim, &["z a", "enter", "space"]),
        (FoldAll, Response, Conventional, &["-"]),
        (FoldAll, Response, Vim, &["z M", "-"]),
        (UnfoldAll, Response, Conventional, &["+", "="]),
        (UnfoldAll, Response, Vim, &["z R", "+", "="]),
        (ToggleWrap, Response, Both, &["w"]),
        (CopyBody, Response, Conventional, &["y", "ctrl+c"]),
        (CopyBody, Response, Vim, &["y y", "ctrl+c"]),
        (SaveBody, Response, Both, &["s"]),
        (NextCell, Table, Both, &["enter"]),
        (ToggleRow, Table, Both, &["ctrl+t"]),
        (DeleteRow, Table, Both, &["ctrl+d"]),
        (Undo, Editor, Conventional, &["ctrl+z"]),
        (Redo, Editor, Conventional, &["ctrl+y"]),
        (SelectAll, Editor, Conventional, &["ctrl+a"]),
        (Copy, Editor, Conventional, &["ctrl+c"]),
        (Cut, Editor, Conventional, &["ctrl+x"]),
        (Paste, Editor, Conventional, &["ctrl+v"]),
        (Complete, Global, Both, &["ctrl+space"]),
    ]
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Action(Action),
    /// The keys so far are a prefix of a longer binding.
    Prefix,
    None,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

impl Keymap {
    pub fn defaults() -> Self {
        let bindings = DEFAULTS
            .iter()
            .map(|(a, c, m, keys)| Binding {
                action: *a,
                ctx: *c,
                modes: *m,
                keys: keys.iter().filter_map(|k| KeySeq::parse(k).ok()).collect(),
            })
            .collect();
        Self { bindings }
    }

    /// Applies `[keymap.<mode>]` overrides. Mode is `all`, `conventional` or `vim`.
    /// Returns warnings for unknown actions, modes or keys.
    pub fn with_overrides(
        mut self,
        cfg: &BTreeMap<String, BTreeMap<String, KeySpec>>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        for (mode, actions) in cfg {
            let modes = match mode.as_str() {
                "all" | "default" => ModeSet::Both,
                "conventional" => ModeSet::Conventional,
                "vim" => ModeSet::Vim,
                o => {
                    warnings.push(format!(
                        "keymap: unknown mode {o:?} (use all, conventional or vim)"
                    ));
                    continue;
                }
            };
            for (id, spec) in actions {
                let Some(action) = Action::from_id(id) else {
                    warnings.push(format!("keymap: unknown action {id:?}"));
                    continue;
                };
                let mut keys = Vec::new();
                for k in spec.keys() {
                    match KeySeq::parse(&k) {
                        Ok(s) => keys.push(s),
                        Err(e) => warnings.push(format!("keymap: {id}: {e}")),
                    }
                }
                let mut found = false;
                for b in self.bindings.iter_mut().filter(|b| b.action == action) {
                    if modes == ModeSet::Both || b.modes == modes || b.modes == ModeSet::Both {
                        if b.modes == ModeSet::Both && modes != ModeSet::Both {
                            continue;
                        }
                        b.keys = keys.clone();
                        found = true;
                    }
                }
                if !found {
                    let ctx = self
                        .bindings
                        .iter()
                        .find(|b| b.action == action)
                        .map(|b| b.ctx)
                        .unwrap_or(Ctx::Global);
                    self.bindings.push(Binding {
                        action,
                        ctx,
                        modes,
                        keys,
                    });
                }
            }
        }
        (self, warnings)
    }

    fn applies(b: &Binding, vim: bool) -> bool {
        match b.modes {
            ModeSet::Both => true,
            ModeSet::Vim => vim,
            ModeSet::Conventional => !vim,
        }
    }

    /// Looks up a key sequence in the given contexts (first match wins).
    pub fn lookup(&self, ctxs: &[Ctx], vim: bool, seq: &[Chord]) -> Lookup {
        let mut prefix = false;
        for ctx in ctxs {
            for b in self
                .bindings
                .iter()
                .filter(|b| b.ctx == *ctx && Self::applies(b, vim))
            {
                for k in &b.keys {
                    if k.0 == seq {
                        return Lookup::Action(b.action);
                    }
                    if k.0.len() > seq.len() && k.0[..seq.len()] == *seq {
                        prefix = true;
                    }
                }
            }
        }
        if prefix { Lookup::Prefix } else { Lookup::None }
    }

    /// Keys for an action in the given mode, formatted (`F5 / Ctrl+Enter`).
    pub fn keys_for(&self, action: Action, vim: bool) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|b| b.action == action && Self::applies(b, vim))
            .flat_map(|b| b.keys.iter().map(|k| k.to_string()))
            .collect()
    }

    /// The first key for an action, for hints.
    pub fn hint(&self, action: Action, vim: bool) -> String {
        self.keys_for(action, vim)
            .into_iter()
            .next()
            .unwrap_or_else(|| "(unbound)".into())
    }

    /// The bindings of `ctxs` that complete `pending` with one more key, in
    /// table order: what the Ctrl+W menu lists. User overrides are included.
    pub fn continuations(
        &self,
        ctxs: &[Ctx],
        vim: bool,
        pending: &[Chord],
    ) -> Vec<(Chord, Action)> {
        let mut out: Vec<(Chord, Action)> = Vec::new();
        for b in self
            .bindings
            .iter()
            .filter(|b| ctxs.contains(&b.ctx) && Self::applies(b, vim))
        {
            for k in &b.keys {
                if k.0.len() == pending.len() + 1
                    && k.0[..pending.len()] == *pending
                    && let Some(last) = k.0.last()
                    && !out.iter().any(|(c, _)| c == last)
                {
                    out.push((*last, b.action));
                }
            }
        }
        out
    }

    /// Rows for the help screen and KEYMAP.md: (context, action, conventional keys, vim keys).
    pub fn rows(&self) -> Vec<(Ctx, Action, String, String)> {
        let mut seen: Vec<(Ctx, Action)> = Vec::new();
        for b in &self.bindings {
            if !seen.contains(&(b.ctx, b.action)) {
                seen.push((b.ctx, b.action));
            }
        }
        seen.sort_by_key(|(c, _)| *c);
        seen.into_iter()
            .map(|(c, a)| {
                let keys = |vim: bool| {
                    self.bindings
                        .iter()
                        .filter(|b| b.ctx == c && b.action == a && Self::applies(b, vim))
                        .flat_map(|b| b.keys.iter().map(|k| k.to_string()))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                (c, a, keys(false), keys(true))
            })
            .collect()
    }
}

/// Short key names for the footer and the Ctrl+W menu: `Ctrl+S` → `^S`,
/// `Enter` → `⏎`, `Shift+Tab` → `⇧Tab`, `PageDown` → `PgDn`.
pub fn short_keys(s: &str) -> String {
    s.replace("Ctrl+", "^")
        .replace("Shift+", "⇧")
        .replace("Enter", "⏎")
        .replace("PageDown", "PgDn")
        .replace("PageUp", "PgUp")
}

/// The prefix key of the window menu, as the footer shows it.
pub const MENU_PREFIX: &str = "Ctrl+W";

/// A column of the Ctrl+W menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MenuGroup {
    Request,
    Tabs,
    Layout,
    Open,
    App,
}

impl MenuGroup {
    pub const ALL: [MenuGroup; 5] = [
        MenuGroup::Request,
        MenuGroup::Tabs,
        MenuGroup::Layout,
        MenuGroup::Open,
        MenuGroup::App,
    ];

    pub fn title(self) -> &'static str {
        match self {
            MenuGroup::Request => "Request",
            MenuGroup::Tabs => "Tabs",
            MenuGroup::Layout => "Focus & layout",
            MenuGroup::Open => "Open",
            MenuGroup::App => "App",
        }
    }
}

/// The Ctrl+W menu group and short label of an action. Actions with the same
/// label share one row ("h j k l  focus"). Actions without an entry go to
/// App with their description.
pub fn menu_entry(a: Action) -> (MenuGroup, &'static str) {
    use Action::*;
    use MenuGroup as G;
    match a {
        Send => (G::Request, "send"),
        Cancel => (G::Request, "cancel"),
        EndStream => (G::Request, "end stream"),
        WatchToggle => (G::Request, "watch"),
        Save => (G::Request, "save"),
        FormatBody => (G::Request, "format body"),
        MoveToSecret => (G::Request, "move to secret"),
        RevealSecrets => (G::Request, "show secrets"),
        ExportCurl => (G::Request, "copy as curl"),
        NewRequest => (G::Tabs, "new HTTP"),
        NewGrpcRequest => (G::Tabs, "new gRPC"),
        NewWsRequest => (G::Tabs, "new WebSocket"),
        NewSseRequest => (G::Tabs, "new SSE"),
        NewMqttRequest => (G::Tabs, "new MQTT"),
        NextTab | PrevTab => (G::Tabs, "next / previous"),
        CloseTab => (G::Tabs, "close"),
        TogglePin => (G::Tabs, "pin"),
        ToggleFavorite => (G::Request, "favorite"),
        FocusLeft | FocusDown | FocusUp | FocusRight => (G::Layout, "focus"),
        FocusNext | FocusPrev => (G::Layout, "next / previous pane"),
        NextSubTab | PrevSubTab => (G::Layout, "pane tab"),
        Zoom => (G::Layout, "zoom"),
        ToggleSidebar => (G::Layout, "sidebar"),
        GrowSidebar | ShrinkSidebar => (G::Layout, "sidebar width"),
        GrowRequest | ShrinkRequest => (G::Layout, "split height"),
        SelectEnv => (G::Open, "environment"),
        ManageEnvs => (G::Open, "edit environments"),
        ManageMcpServers => (G::Open, "MCP servers"),
        History => (G::Open, "history"),
        Import => (G::Open, "import"),
        Settings => (G::App, "settings"),
        Help => (G::App, "all keys"),
        Quit => (G::App, "quit"),
        other => (G::App, other.description()),
    }
}

/// Keys handled directly by widgets (not remappable), documented in help and KEYMAP.md.
pub const FIXED_KEYS: &[(&str, &str, &str)] = &[
    (
        "Text editing",
        "Arrows, Home, End, PageUp, PageDown",
        "Move the cursor",
    ),
    (
        "Text editing",
        "Shift+Arrows, Shift+Home/End",
        "Extend the selection",
    ),
    ("Text editing", "Ctrl+Left / Ctrl+Right", "Move by word"),
    (
        "Text editing",
        "Backspace / Delete",
        "Delete before / after the cursor",
    ),
    (
        "Text editing",
        "Tab (body editor)",
        "Insert two spaces (Esc first to leave the editor)",
    ),
    (
        "Text editing",
        "Esc",
        "Leave the editor (focus goes to the tab bar)",
    ),
    (
        "Text editing",
        "Terminal paste (e.g. Ctrl+Shift+V / Cmd+V)",
        "Paste from the system clipboard",
    ),
    (
        "Tab bars",
        "Left / Right",
        "Switch tab when a tab bar is focused",
    ),
    ("Tab bars", "Enter / Down", "Go into the tab's content"),
    (
        "Method / pickers",
        "Enter or Space",
        "Open the picker; type to filter, Enter to choose, Esc to close",
    ),
    (
        "Dialogs",
        "Enter / Esc",
        "Input dialogs: Enter confirms from any field (Ctrl+S too); Tab moves between fields; on a choice row Left/Right change the value and Space opens its list. Question dialogs: Enter presses the focused button; the letter in a button (y, s, d) presses it directly",
    ),
    (
        "History (sidebar)",
        "Enter",
        "Open a copy of the entry in a new tab",
    ),
    (
        "History (sidebar)",
        "Ctrl+Enter / F5",
        "Resend the entry (opens a copy and sends it)",
    ),
    ("History (sidebar)", "s", "Save the entry to the tree"),
    ("History (sidebar)", "F7", "Back to the tree"),
    (
        "Tables",
        "Space (on a ‹choice› cell: multipart Kind, env Source)",
        "Change the value",
    ),
    (
        "Environment editor",
        "Space (Source cell)",
        "Change where the value lives: plain, secrets (.sendito/secrets.toml), keychain, file",
    ),
    (
        "Environment editor",
        "Ctrl+S / Ctrl+N / Ctrl+PageUp, Ctrl+PageDown",
        "Save / new environment / previous, next environment",
    ),
    (
        "Environment editor",
        "Enter / Ctrl+D / Esc",
        "Next cell / delete the row / close",
    ),
    (
        "Import dialog",
        "F5 or Ctrl+S / Esc",
        "Import the pasted curl command or collection / cancel",
    ),
    (
        "Settings",
        "Up, Down / Left, Right or Space / Esc",
        "Choose a setting / change its value / close",
    ),
    (
        "Report dialogs",
        "y / Up, Down, PageUp, PageDown / Esc",
        "Copy / scroll / close",
    ),
    (
        "MCP server / tool field",
        "F2",
        "Choose a configured server / discover and choose a tool",
    ),
    (
        "Command palette",
        ":<command>",
        "Type a command (listed under Commands) or pick from the list",
    ),
    ("Vim mode", "i a I A o O", "Enter insert mode"),
    ("Vim mode", "Esc", "Back to normal mode"),
    (
        "Vim mode",
        "h j k l  w b e  0 ^ $  gg G",
        "Motions (normal and visual)",
    ),
    (
        "Vim mode",
        "x  dd  D  cc  C  yy  p  P  u  Ctrl+R",
        "Edit, yank, put, undo, redo",
    ),
    (
        "Vim mode",
        "d c y + a motion (dw, cw, d$, yw, dG …)",
        "Delete / change / yank over the motion",
    ),
    ("Vim mode", "v  V", "Visual / visual line; then d, y, c"),
    ("Vim mode", ":", "Command palette in command mode"),
    ("Vim mode", "/", "Search (response) or filter (tree)"),
    (
        "Mouse",
        "Click",
        "Focus panes, select rows, switch and close tabs, press buttons",
    ),
    ("Mouse", "Double-click", "Open a request from the tree"),
    ("Mouse", "Drag a tree item onto a folder", "Move it"),
    (
        "Mouse",
        "Drag a pane border",
        "Resize the sidebar or the request/response split",
    ),
    ("Mouse", "Wheel", "Scroll the pane under the pointer"),
];

/// Symbols used on screen, listed in the help and KEYMAP.md.
pub const SYMBOLS: &[(&str, &str)] = &[
    ("●", "unsaved changes (tab, status line)"),
    ("⠋ on a tab (turning)", "request running"),
    (
        "Results 2/3 ✗",
        "on the Results tab: assertions passed / total (✓ all passed, ✗ some failed)",
    ),
    ("…", "text cut short"),
    (
        "→ ← ■ + in the gRPC and WebSocket Log tabs",
        "sent, received, final status, seconds since the session started",
    ),
    (
        "Headers (2)",
        "number of rows in a tab (compact: Headers(2))",
    ),
    ("Body•  Auth•", "the body or the auth is set"),
    ("▾", "opens a menu or picker"),
    (
        "‹value›",
        "a choice: Space (or ← → in dialogs and settings) changes it",
    ),
    ("▸ / ▾ in the tree", "closed / open folder"),
    ("italic request in the tree", "open in a tab"),
    ("✕", "delete this row"),
    ("[x] / [ ]", "row enabled / disabled"),
    (
        "{{name}} in red",
        "variable not defined in the active environment",
    ),
    ("••••••", "a secret value, masked"),
];

/// Renders KEYMAP.md from the default key map.
pub fn keymap_markdown() -> String {
    let km = Keymap::defaults();
    let mut out = String::new();
    out.push_str("# Sendito key map\n\n");
    out.push_str(
        "Generated from the key table in `crates/ui/src/keymap.rs` (`sendito --print-keymap`).\n",
    );
    out.push_str("The in-app help (F1) shows the same table, including your overrides.\n\n");
    out.push_str("Conventional mode is the default. Vim mode is enabled in Settings (`Ctrl+W ,`) or with\n`[ui] vim_mode = true`. `Ctrl+W x` means: press Ctrl+W, release, then press x.\n\n");
    out.push_str("Ctrl+Enter needs a terminal that reports it (kitty, Alacritty, WezTerm, foot, Ghostty,\niTerm2 with CSI u). F5 and `Ctrl+W e` work everywhere.\n\n");
    out.push_str("When a key is bound both in the focused pane's section and in Global, the pane's\nbinding wins: in vim mode `Ctrl+B` pages up in the response viewer and toggles the\nsidebar elsewhere (`Ctrl+W b` toggles it everywhere).\n\n");
    let mut current: Option<Ctx> = None;
    for (ctx, action, conv, vim) in km.rows() {
        if current != Some(ctx) {
            current = Some(ctx);
            out.push_str(&format!("\n## {}\n\n", ctx.title()));
            out.push_str("| Action | Conventional | Vim mode | Description |\n|---|---|---|---|\n");
        }
        let cell = |s: &str| {
            if s.is_empty() {
                "—".to_string()
            } else {
                format!("`{}`", s.replace(", ", "`, `").replace('|', "\\|"))
            }
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            action.id(),
            cell(&conv),
            cell(&vim),
            action.description()
        ));
    }
    out.push_str("\n## Fixed keys\n\nThese are handled by the widgets themselves and cannot be remapped.\n\n| Where | Keys | What |\n|---|---|---|\n");
    for (w, k, d) in FIXED_KEYS {
        out.push_str(&format!("| {w} | {} | {d} |\n", k.replace('|', "\\|")));
    }
    out.push_str("\n## Symbols\n\n| Symbol | Meaning |\n|---|---|\n");
    for (s, m) in SYMBOLS {
        out.push_str(&format!("| {s} | {m} |\n"));
    }
    out.push_str("\n## Commands\n\nTyped in the command palette (`Ctrl+P`, or `:` in vim normal mode).\n\n| Command | What |\n|---|---|\n");
    for (c, d) in crate::overlay::COMMANDS {
        out.push_str(&format!("| `{c}` | {d} |\n"));
    }
    out.push_str("\n## Customizing\n\nOverride any action in `~/.config/sendito/config.toml` or `<collection>/.sendito/config.toml`.\nSections are `[keymap.all]`, `[keymap.conventional]` and `[keymap.vim]`; values are a key or a list of keys.\n\n```toml\n[keymap.all]\nsend = [\"f9\", \"ctrl+w e\"]\n\n[keymap.vim]\ntree_delete = \"D\"\n```\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_key_names() {
        assert_eq!(short_keys("Ctrl+S"), "^S");
        assert_eq!(short_keys("Ctrl+Enter/F5"), "^⏎/F5");
        assert_eq!(short_keys("Shift+Tab"), "⇧Tab");
        assert_eq!(short_keys("Ctrl+PageDown"), "^PgDn");
        assert_eq!(short_keys("F1"), "F1");
    }

    #[test]
    fn ctrl_w_continuations_follow_overrides() {
        let ctrl_w = Chord::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let e = Chord::new(KeyCode::Char('e'), KeyModifiers::NONE);
        let km = Keymap::defaults();
        let c = km.continuations(&[Ctx::Global], false, &[ctrl_w]);
        assert!(c.contains(&(e, Action::Send)));
        // A key that ends a longer sequence is listed once.
        assert_eq!(c.iter().filter(|(k, _)| *k == e).count(), 1);
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "all".to_string(),
            BTreeMap::from([("send".to_string(), KeySpec::One("ctrl+w o".into()))]),
        );
        let (km, warnings) = km.with_overrides(&cfg);
        assert!(warnings.is_empty(), "{warnings:?}");
        let c = km.continuations(&[Ctx::Global], false, &[ctrl_w]);
        let o = Chord::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert!(c.contains(&(o, Action::Send)));
        assert!(!c.contains(&(e, Action::Send)));
        // Vim prefixes such as `g` have no global continuations.
        let g = Chord::new(KeyCode::Char('g'), KeyModifiers::NONE);
        assert!(km.continuations(&[Ctx::Global], true, &[g]).is_empty());
    }

    #[test]
    fn filter_key_by_mode() {
        // The jq filter is j in conventional mode and J in vim mode (j moves
        // the cursor there); plain j must not open it in vim mode.
        let km = Keymap::defaults();
        let j = Chord::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let upper = Chord::new(KeyCode::Char('J'), KeyModifiers::NONE);
        assert_eq!(
            km.lookup(&[Ctx::Response], false, &[j]),
            Lookup::Action(Action::Filter)
        );
        assert_eq!(
            km.lookup(&[Ctx::Response], true, &[j]),
            Lookup::Action(Action::CursorDown)
        );
        assert_eq!(
            km.lookup(&[Ctx::Response], true, &[upper]),
            Lookup::Action(Action::Filter)
        );
        // And the hint follows the mode.
        assert_eq!(km.hint(Action::Filter, false), "j");
        assert_eq!(km.hint(Action::Filter, true), "J");
    }

    #[test]
    fn parse_and_display() {
        for (s, d) in [
            ("ctrl+w", "Ctrl+W"),
            ("f5", "F5"),
            ("ctrl+enter", "Ctrl+Enter"),
            ("shift+tab", "Shift+Tab"),
            ("alt+1", "Alt+1"),
            ("?", "?"),
            ("N", "N"),
            ("space", "Space"),
            ("+", "+"),
            ("ctrl++", "Ctrl++"),
            ("ctrl+pagedown", "Ctrl+PageDown"),
            ("backtab", "Shift+Tab"),
        ] {
            assert_eq!(
                Chord::parse(s).map(|c| c.to_string()),
                Ok(d.to_string()),
                "{s}"
            );
        }
        assert!(Chord::parse("hyper+x").is_err());
        assert!(Chord::parse("f13").is_err());
        assert!(Chord::parse("bogus").is_err());
        assert!(KeySeq::parse("").is_err());
        assert_eq!(
            KeySeq::parse("ctrl+w h").map(|k| k.to_string()),
            Ok("Ctrl+W h".into())
        );
        // Shifted letters and BackTab normalize.
        assert_eq!(
            Chord::new(KeyCode::Char('n'), KeyModifiers::SHIFT),
            Chord::parse("N").unwrap_or(Chord::new(KeyCode::Null, KeyModifiers::NONE))
        );
        assert_eq!(
            Chord::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            Chord::parse("shift+tab").unwrap_or(Chord::new(KeyCode::Null, KeyModifiers::NONE))
        );
        assert_eq!(
            Chord::new(
                KeyCode::Char('W'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
            .to_string(),
            "Ctrl+W"
        );
    }

    #[test]
    fn lookup_and_overrides() {
        let km = Keymap::defaults();
        let c = |s: &str| KeySeq::parse(s).map(|k| k.0).unwrap_or_default();
        assert_eq!(
            km.lookup(&[Ctx::Global], false, &c("f5")),
            Lookup::Action(Action::Send)
        );
        assert_eq!(
            km.lookup(&[Ctx::Global], false, &c("ctrl+w")),
            Lookup::Prefix
        );
        assert_eq!(
            km.lookup(&[Ctx::Global], false, &c("ctrl+w e")),
            Lookup::Action(Action::Send)
        );
        assert_eq!(
            km.lookup(&[Ctx::Tree, Ctx::Global], true, &c("j")),
            Lookup::Action(Action::CursorDown)
        );
        assert_eq!(
            km.lookup(&[Ctx::Tree, Ctx::Global], false, &c("j")),
            Lookup::None
        );
        assert_eq!(km.lookup(&[Ctx::Response], true, &c("g")), Lookup::Prefix);
        let mut cfg = BTreeMap::new();
        let mut all = BTreeMap::new();
        all.insert("send".to_string(), KeySpec::Many(vec!["f9".into()]));
        all.insert("nope".to_string(), KeySpec::One("f1".into()));
        all.insert("quit".to_string(), KeySpec::One("hyper+q".into()));
        cfg.insert("all".to_string(), all);
        let mut vim = BTreeMap::new();
        vim.insert("tree_delete".to_string(), KeySpec::One("D".into()));
        vim.insert("cursor".to_string(), KeySpec::One("x".into()));
        cfg.insert("vim".to_string(), vim);
        cfg.insert("bogus".to_string(), BTreeMap::new());
        let (km, w) = km.with_overrides(&cfg);
        assert_eq!(w.len(), 4, "{w:?}");
        assert_eq!(
            km.lookup(&[Ctx::Global], false, &c("f9")),
            Lookup::Action(Action::Send)
        );
        assert_eq!(km.lookup(&[Ctx::Global], false, &c("f5")), Lookup::None);
        assert_eq!(
            km.lookup(&[Ctx::Tree], true, &c("D")),
            Lookup::Action(Action::TreeDelete)
        );
        assert_eq!(km.hint(Action::Send, false), "F9");
        assert!(
            km.keys_for(Action::TreeDelete, false)
                .contains(&"Delete".to_string())
        );
    }

    #[test]
    fn no_key_is_bound_twice() {
        // A duplicate would silently shadow the earlier action in `lookup`.
        let km = Keymap::defaults();
        let mut seen: Vec<(Ctx, ModeSet, KeySeq)> = Vec::new();
        for b in &km.bindings {
            for k in &b.keys {
                if let Some((ctx, modes, _)) = seen
                    .iter()
                    .find(|(c, m, s)| *c == b.ctx && *m == b.modes && s == k)
                {
                    panic!(
                        "{k} is bound to two actions in {ctx:?} ({modes:?}): {:?}",
                        b.action
                    );
                }
                seen.push((b.ctx, b.modes, k.clone()));
            }
        }
    }

    #[test]
    fn every_action_bound_and_documented() {
        let km = Keymap::defaults();
        for a in Action::ALL {
            assert!(
                !km.keys_for(*a, false).is_empty() || !km.keys_for(*a, true).is_empty(),
                "{a:?} unbound"
            );
        }
        let md = keymap_markdown();
        for a in Action::ALL {
            assert!(
                md.contains(&format!("`{}`", a.id())),
                "{a:?} missing from KEYMAP.md"
            );
        }
    }

    #[test]
    fn keymap_md_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../KEYMAP.md");
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        if std::env::var("UPDATE_KEYMAP").is_ok() {
            std::fs::write(&path, keymap_markdown()).unwrap();
            return;
        }
        assert_eq!(
            on_disk,
            keymap_markdown(),
            "KEYMAP.md is stale: run UPDATE_KEYMAP=1 cargo test -p sendito-ui keymap_md_is_current"
        );
    }
}
