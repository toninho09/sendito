//! Keys for each overlay dialog.

use super::*;

impl App {
    // ----- overlay input ------------------------------------------------

    pub(super) fn overlay_key(&mut self, key: KeyEvent) {
        let Some(mut ov) = self.overlay.take() else {
            return;
        };
        let keep = self.overlay_key_inner(&mut ov, key);
        if keep && self.overlay.is_none() {
            self.overlay = Some(ov);
        }
    }

    /// Returns whether the overlay stays open.
    pub(super) fn overlay_key_inner(&mut self, ov: &mut Overlay, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match ov {
            Overlay::Report(r) => self.report_key(r, key),
            Overlay::Help { viewer, filter } => self.help_key(viewer, filter, key),
            Overlay::Settings(v) => self.settings_key(v, key),
            Overlay::Confirm(c) => self.confirm_key(c, key),
            Overlay::Picker(p) => self.picker_key(p, key),
            Overlay::Palette(p) => self.palette_key(p, key),
            Overlay::Prompt(p) => self.prompt_key(p, key, ctrl),
            Overlay::Import(i) => self.import_key(i, key, ctrl),
            Overlay::Env(e) => self.env_key(e, key, ctrl),
            Overlay::Records(e) => self.records_key(e, key),
            Overlay::Keymap(e) => self.keymap_key(e, key),
        }
    }

    pub(super) fn report_key(&mut self, r: &mut Report, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                if let Some(m) = r.close_status.clone() {
                    self.set_status(m, StatusKind::Ok);
                }
                false
            }
            KeyCode::Char('y') => {
                self.clipboard.set(r.copy_text.clone(), false);
                self.osc52.push(r.copy_text.clone());
                r.copied = true;
                self.set_status("copied (sent to the terminal clipboard)", StatusKind::Ok);
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                r.viewer.move_cursor(-1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                r.viewer.move_cursor(1);
                true
            }
            KeyCode::PageUp => {
                r.viewer.page(-1);
                true
            }
            KeyCode::PageDown => {
                r.viewer.page(1);
                true
            }
            _ => true,
        }
    }

    pub(super) fn help_key(
        &mut self,
        viewer: &mut Viewer,
        filter: &mut TextEditor,
        key: KeyEvent,
    ) -> bool {
        match key.code {
            KeyCode::Esc => false,
            KeyCode::Up => {
                viewer.move_cursor(-1);
                true
            }
            KeyCode::Down => {
                viewer.move_cursor(1);
                true
            }
            KeyCode::PageUp => {
                viewer.page(-1);
                true
            }
            KeyCode::PageDown => {
                viewer.page(1);
                true
            }
            KeyCode::F(1) => false,
            _ => {
                if filter.handle_key(&key, false, &mut self.clipboard) == Outcome::Changed {
                    let q = filter.text();
                    viewer.search(&q);
                }
                if key.code == KeyCode::Enter {
                    viewer.next_match(1);
                }
                true
            }
        }
    }

    pub(super) fn settings_key(&mut self, v: &mut SettingsView, key: KeyEvent) -> bool {
        let id = SETTINGS[v.sel].0;
        // A value being typed in place: Enter saves it, Esc gives it up.
        if let Some(ed) = &mut v.edit {
            match key.code {
                KeyCode::Esc => {
                    v.edit = None;
                    v.error = None;
                }
                KeyCode::Enter => match self.set_typed_setting(id, &ed.text()) {
                    Ok(()) => {
                        v.edit = None;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                },
                _ => {
                    ed.handle_key(&key, false, &mut self.clipboard);
                    v.error = None;
                }
            }
            return true;
        }
        match key.code {
            KeyCode::Esc if !v.filter.text().is_empty() => {
                v.filter.set_text("");
                true
            }
            KeyCode::Esc => false,
            KeyCode::Up => {
                v.step(-1);
                true
            }
            KeyCode::Down => {
                v.step(1);
                true
            }
            KeyCode::PageUp => {
                v.step(-8);
                true
            }
            KeyCode::PageDown => {
                v.step(8);
                true
            }
            // Tab switches the file the changes go to.
            KeyCode::Tab | KeyCode::BackTab => {
                self.change_setting(SettingId::Scope, 1);
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Left if id.typed() => {
                v.start_edit(self.setting_raw(id));
                true
            }
            KeyCode::Left => {
                self.change_setting(id, -1);
                true
            }
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                self.change_setting(id, 1);
                true
            }
            _ => {
                if v.filter.handle_key(&key, false, &mut self.clipboard)
                    == crate::editor::Outcome::Changed
                {
                    v.clamp();
                }
                true
            }
        }
    }

    pub(super) fn confirm_key(&mut self, c: &mut Confirm, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                // Cancelling is an answer too: it is the last button, and
                // it restores whatever the dialog was guarding (an editor,
                // a tab).
                let action = c.action.clone();
                let cancel = c.buttons.len().saturating_sub(1);
                self.confirm(action, cancel);
                false
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                c.sel = crate::list::step(c.sel, -1, c.buttons.len());
                true
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                c.sel = crate::list::step(c.sel, 1, c.buttons.len());
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let (action, sel) = (c.action.clone(), c.sel);
                self.confirm(action, sel);
                false
            }
            KeyCode::Char('y') if c.buttons.len() == 2 => {
                let action = c.action.clone();
                self.confirm(action, 0);
                false
            }
            KeyCode::Char('s') if c.buttons.len() == 3 => {
                let action = c.action.clone();
                self.confirm(action, 0);
                false
            }
            KeyCode::Char('d') if c.buttons.len() == 3 => {
                let action = c.action.clone();
                self.confirm(action, 1);
                false
            }
            KeyCode::Char('n') if c.buttons.len() == 2 => {
                let action = c.action.clone();
                self.confirm(action, 1);
                false
            }
            _ => true,
        }
    }

    pub(super) fn picker_key(&mut self, p: &mut Picker, key: KeyEvent) -> bool {
        {
            let vis = p.visible();
            match key.code {
                // The env editor waits behind its file picker.
                KeyCode::Esc => {
                    if matches!(p.action, PickAction::EnvFile { .. })
                        && let Some(ed) = self.parked_env.take()
                    {
                        self.overlay = Some(Overlay::Env(ed));
                    }
                    false
                }
                KeyCode::Up => {
                    p.sel = crate::list::step(p.sel, -1, vis.len());
                    true
                }
                KeyCode::Down => {
                    p.sel = crate::list::step(p.sel, 1, vis.len());
                    true
                }
                KeyCode::PageUp => {
                    p.sel = crate::list::page(p.sel, -1, 10, vis.len());
                    true
                }
                KeyCode::PageDown => {
                    p.sel = crate::list::page(p.sel, 1, 10, vis.len());
                    true
                }
                KeyCode::Enter => {
                    if let Some(i) = p.selected() {
                        let action = p.action.clone();
                        let item = p.items[i].clone();
                        let detail = p.details.get(i).cloned();
                        self.pick(action, i, &item, detail);
                    }
                    false
                }
                _ => {
                    if p.filter.handle_key(&key, false, &mut self.clipboard) == Outcome::Changed {
                        p.sel = 0;
                    }
                    true
                }
            }
        }
    }

    /// Puts `:<base> ` in the palette input so the argument can be typed.
    pub(super) fn prefill_command(p: &mut Palette, base: &str) {
        let mut input = TextEditor::single(&format!(":{base} "));
        input.move_to_end();
        p.input = input;
        p.sel = 0;
    }

    pub(super) fn palette_key(&mut self, p: &mut Palette, key: KeyEvent) -> bool {
        {
            let vis = p.visible();
            match key.code {
                KeyCode::Esc => false,
                KeyCode::Up => {
                    p.sel = crate::list::step(p.sel, -1, vis.len());
                    p.armed = true;
                    true
                }
                KeyCode::Down | KeyCode::Tab => {
                    // The first move highlights the first row.
                    if p.armed {
                        p.sel = crate::list::step(p.sel, 1, vis.len());
                    }
                    p.armed = true;
                    true
                }
                KeyCode::Enter if !p.armed => true,
                KeyCode::Enter => {
                    let text = p.input.text();
                    let t = text.trim();
                    if t.starts_with(':')
                        && let Some(i) = vis.get(p.sel)
                        && let PaletteItem::Env(e) = &p.items[*i].0
                    {
                        let e = e.clone();
                        self.set_env(e);
                        return false;
                    }
                    if t.starts_with(':') {
                        // A full command with arguments runs as typed; otherwise the selection.
                        let typed_word = t
                            .trim_start_matches(':')
                            .split_whitespace()
                            .next()
                            .unwrap_or("");
                        let exact = COMMANDS.iter().any(|(c, _)| {
                            c.trim_start_matches(':').split_whitespace().next() == Some(typed_word)
                        });
                        if exact || vis.is_empty() {
                            self.run_command(t);
                        } else if let Some(i) = vis.get(p.sel)
                            && let PaletteItem::Command(c, _) = p.items[*i].0
                        {
                            let (base, _) = command_base(c);
                            if c.contains('<') || c.contains('[') {
                                Self::prefill_command(p, base);
                                return true;
                            }
                            self.run_command(base);
                        }
                        return false;
                    }
                    if let Some(i) = vis.get(p.sel) {
                        let item = p.items[*i].0.clone();
                        match item {
                            PaletteItem::Action(a) => self.dispatch(a),
                            PaletteItem::Open(rel) => {
                                self.tree.select_rel(&rel);
                                self.open_request(&rel);
                            }
                            PaletteItem::Env(e) => self.set_env(e),
                            PaletteItem::Setting(id) => self.open_settings(id),
                            PaletteItem::Command(c, _) => {
                                // A command that needs an argument waits for it.
                                let (base, needs_arg) = command_base(c);
                                if needs_arg {
                                    Self::prefill_command(p, base);
                                    return true;
                                }
                                self.run_command(base);
                            }
                        }
                    }
                    false
                }
                _ => {
                    let out = p.input.handle_key(&key, false, &mut self.clipboard);
                    if out == Outcome::Changed {
                        p.sel = 0;
                        p.armed = !p.input.text().trim().is_empty();
                    }
                    // Backspace on an empty input closes the palette.
                    !(out == Outcome::Handled
                        && key.code == KeyCode::Backspace
                        && p.input.text().is_empty())
                }
            }
        }
    }

    pub(super) fn prompt_key(&mut self, p: &mut Prompt, key: KeyEvent, ctrl: bool) -> bool {
        {
            // The open option list of a choice row takes the keys first.
            if let Some(hl) = p.open {
                let n = p
                    .choices
                    .get(p.focus.wrapping_sub(1))
                    .map_or(0, |c| c.1.len());
                match key.code {
                    KeyCode::Up => p.open = Some((hl + n.max(1) - 1) % n.max(1)),
                    KeyCode::Down | KeyCode::Tab => p.open = Some((hl + 1) % n.max(1)),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if let Some(c) = p.choices.get_mut(p.focus - 1) {
                            c.2 = hl.min(n.saturating_sub(1));
                        }
                        p.open = None;
                        // Back to the name, so the next Enter confirms.
                        p.focus = 0;
                    }
                    KeyCode::Esc => p.open = None,
                    _ => {}
                }
                return true;
            }
            // Ctrl+S / F5 confirm from any row (Enter on a choice row opens its list).
            let confirm_any =
                matches!(key.code, KeyCode::F(5)) || (ctrl && key.code == KeyCode::Char('s'));
            if confirm_any {
                return match self.prompt_ok(p) {
                    Ok(()) => false,
                    Err(e) => {
                        p.error = Some(e);
                        true
                    }
                };
            }
            match key.code {
                // A "new environment" prompt was opened from the env
                // editor, which is parked behind it: cancelling returns
                // to the editor with its changes intact.
                KeyCode::Esc => {
                    if matches!(p.action, PromptAction::NewEnv)
                        && let Some(ed) = self.parked_env.take()
                    {
                        self.overlay = Some(Overlay::Env(ed));
                    }
                    false
                }
                // Space opens a choice row's list; Enter always confirms.
                KeyCode::Char(' ') if p.focus > 0 => {
                    p.open = p.choices.get(p.focus - 1).map(|c| c.2);
                    true
                }
                KeyCode::Tab | KeyCode::Down if !p.choices.is_empty() => {
                    p.focus = (p.focus + 1) % (p.choices.len() + 1);
                    true
                }
                KeyCode::BackTab | KeyCode::Up if !p.choices.is_empty() => {
                    p.focus = (p.focus + p.choices.len()) % (p.choices.len() + 1);
                    true
                }
                KeyCode::Left | KeyCode::Right if p.focus > 0 => {
                    if let Some((_, opts, i)) = p.choices.get_mut(p.focus - 1) {
                        let dir: isize = if key.code == KeyCode::Left { -1 } else { 1 };
                        *i = crate::list::cycle(*i, dir, opts.len().max(1));
                    }
                    true
                }
                KeyCode::Enter => match self.prompt_ok(p) {
                    Ok(()) => false,
                    Err(e) => {
                        p.error = Some(e);
                        true
                    }
                },
                _ => {
                    if p.focus == 0 {
                        p.input.handle_key(&key, false, &mut self.clipboard);
                        p.error = None;
                        p.note = None;
                    }
                    true
                }
            }
        }
    }

    pub(super) fn import_key(&mut self, i: &mut Import, key: KeyEvent, ctrl: bool) -> bool {
        {
            let submit = matches!(key.code, KeyCode::F(5))
                || (ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Enter));
            if key.code == KeyCode::Esc {
                return false;
            }
            if submit {
                let text = i.input.text();
                return match self.do_import(&text) {
                    Ok(()) => false,
                    Err(e) => {
                        i.error = Some(e);
                        true
                    }
                };
            }
            if key.code == KeyCode::Tab && key.modifiers.is_empty() {
                if let Some(done) = self.complete_import_path(&i.input.text()) {
                    i.input.set_text(&done);
                    i.input.move_to_end();
                }
                return true;
            }
            i.input.handle_key(&key, false, &mut self.clipboard);
            i.error = None;
            true
        }
    }

    pub(super) fn env_key(&mut self, e: &mut EnvEditor, key: KeyEvent, ctrl: bool) -> bool {
        {
            if key.code == KeyCode::Esc {
                if e.dirty {
                    // Keep the editor open behind the confirm dialog.
                    let ed = std::mem::take(e);
                    self.parked_env = Some(ed);
                    self.overlay = Some(Overlay::Confirm(Confirm {
                        title: "Unsaved environment changes".into(),
                        message: "Save the changes to this environment?".into(),
                        buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                        sel: 0,
                        action: ConfirmAction::EnvClose,
                    }));
                    return false;
                }
                return false;
            }
            if ctrl && key.code == KeyCode::Char('s') {
                let mut ed = std::mem::take(e);
                self.save_env_editor(&mut ed);
                *e = ed;
                return true;
            }
            if ctrl && key.code == KeyCode::Char('n') {
                // Creating an environment loads it into the editor: unsaved
                // edits would be lost, as when switching.
                if e.dirty {
                    e.error = Some("save (Ctrl+S) or discard changes before creating one".into());
                    return true;
                }
                let ed = std::mem::take(e);
                self.parked_env = Some(ed);
                self.prompt_new_env();
                return false;
            }
            if ctrl && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                if e.dirty {
                    e.error = Some("save (Ctrl+S) or discard changes before switching".into());
                    return true;
                }
                let d: isize = if key.code == KeyCode::PageUp { -1 } else { 1 };
                e.current = crate::list::cycle(e.current, d, e.envs.len());
                let mut ed = std::mem::take(e);
                self.load_env_into(&mut ed);
                *e = ed;
                return true;
            }
            let on_file =
                e.table.sel_col == 2 && e.selected_type() == Some(crate::overlay::env_editor::FILE);
            if key.code == KeyCode::F(2) && (e.on_dotenv || on_file) {
                let ed = std::mem::take(e);
                self.open_env_file_picker(ed);
                return false;
            }
            if e.on_dotenv {
                self.env_dotenv_key(e, key);
                return true;
            }
            if ctrl && key.code == KeyCode::Char('d') {
                if e.table.delete_row() {
                    e.dirty = true;
                }
                return true;
            }
            if key.code == KeyCode::Enter {
                e.table.next_cell();
                return true;
            }
            match e.table.handle_key(&key, false, &mut self.clipboard) {
                // Up on the first row goes to the `.env` file line.
                TableOutcome::ExitUp => {
                    e.on_dotenv = true;
                    e.dotenv.move_to_end();
                }
                TableOutcome::Changed => {
                    e.dirty = true;
                    // A new type changes what the row's cells mean.
                    if e.table.sel_col == 1 {
                        self.refresh_env_rows(e);
                    }
                }
                TableOutcome::Leave => return false,
                _ => {}
            }
            true
        }
    }

    /// A key on the `.env` file line of the environment editor. Tab
    /// completes the path; when there is nothing to complete, Tab, Down and
    /// Enter go to the table.
    fn env_dotenv_key(&mut self, e: &mut EnvEditor, key: KeyEvent) {
        let text = e.dotenv.text();
        let done = match key.code {
            KeyCode::Tab if key.modifiers.is_empty() => {
                super::import_export::complete_path(&text, |t| {
                    sendito_store::env::expand(&self.project.root, t)
                })
                .filter(|d| *d != text)
            }
            _ => None,
        };
        if let Some(done) = done {
            e.dotenv.set_text(&done);
            e.dotenv.move_to_end();
        } else if matches!(key.code, KeyCode::Tab | KeyCode::Down | KeyCode::Enter) {
            e.on_dotenv = false;
            return;
        } else {
            e.dotenv.handle_key(&key, false, &mut self.clipboard);
        }
        if e.dotenv.text() != text {
            e.dirty = true;
            self.refresh_env_rows(e);
        }
    }
}
