//! Form widgets used in the request editor: an editable table with text,
//! choice and toggle columns (params, headers, form fields, multipart parts,
//! scripts, metadata).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::{Clipboard, Outcome, TextEditor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColKind {
    Text,
    Choice(&'static [&'static str]),
    Toggle,
}

#[derive(Debug, Clone, Copy)]
pub struct Column {
    pub title: &'static str,
    pub kind: ColKind,
    /// Relative width weight for text columns.
    pub weight: u16,
}

#[derive(Debug, Clone)]
pub enum Cell {
    Text(TextEditor),
    Choice(usize),
    Toggle(bool),
}

impl Cell {
    pub fn text(&self) -> String {
        match self {
            Cell::Text(e) => e.text(),
            Cell::Choice(i) => i.to_string(),
            Cell::Toggle(b) => b.to_string(),
        }
    }

    pub fn choice(&self) -> usize {
        match self {
            Cell::Choice(i) => *i,
            _ => 0,
        }
    }

    pub fn toggle(&self) -> bool {
        matches!(self, Cell::Toggle(true))
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub enabled: bool,
    pub cells: Vec<Cell>,
}

/// What a table key press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableOutcome {
    Handled,
    Changed,
    /// Up on the first row: move focus to the previous field.
    ExitUp,
    /// Down on the trailing empty row: move focus to the next field.
    ExitDown,
    Ignored,
    Leave,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Selected row; `rows.len()` is the trailing "add" row.
    pub sel_row: usize,
    pub sel_col: usize,
    pub scroll: usize,
    /// Show the enable checkbox column.
    pub has_enable: bool,
    pub placeholder: Vec<&'static str>,
}

impl Table {
    pub fn new(columns: Vec<Column>, has_enable: bool) -> Self {
        let placeholder = columns.iter().map(|c| c.title).collect();
        Self {
            columns,
            rows: Vec::new(),
            sel_row: 0,
            sel_col: 0,
            scroll: 0,
            has_enable,
            placeholder,
        }
    }

    pub fn kv(name: &'static str, value: &'static str) -> Self {
        Self::new(
            vec![
                Column {
                    title: name,
                    kind: ColKind::Text,
                    weight: 2,
                },
                Column {
                    title: value,
                    kind: ColKind::Text,
                    weight: 3,
                },
            ],
            true,
        )
    }

    pub fn new_row(&self) -> Row {
        Row {
            enabled: true,
            cells: self
                .columns
                .iter()
                .map(|c| match c.kind {
                    ColKind::Text => Cell::Text(TextEditor::single("")),
                    ColKind::Choice(_) => Cell::Choice(0),
                    ColKind::Toggle => Cell::Toggle(false),
                })
                .collect(),
        }
    }

    pub fn push(&mut self, enabled: bool, cells: Vec<Cell>) {
        self.rows.push(Row { enabled, cells });
    }

    pub fn push_texts(&mut self, enabled: bool, texts: &[&str]) {
        let cells = texts
            .iter()
            .map(|t| Cell::Text(TextEditor::single(t)))
            .collect();
        self.push(enabled, cells);
    }

    /// Rows as (enabled, cells) skipping fully empty rows.
    pub fn values(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| {
                r.cells
                    .iter()
                    .any(|c| matches!(c, Cell::Text(e) if !e.text().is_empty()))
            })
            .collect()
    }

    pub fn cell_mut(&mut self) -> Option<&mut Cell> {
        self.rows.get_mut(self.sel_row)?.cells.get_mut(self.sel_col)
    }

    pub fn editor_mut(&mut self) -> Option<&mut TextEditor> {
        match self.cell_mut()? {
            Cell::Text(e) => Some(e),
            _ => None,
        }
    }

    fn ensure_row(&mut self) {
        if self.sel_row >= self.rows.len() {
            let r = self.new_row();
            self.rows.push(r);
            self.sel_row = self.rows.len() - 1;
        }
    }

    fn drop_trailing_empty(&mut self) {
        // A row the user just left without typing anything is discarded.
        while let Some(last) = self.rows.last() {
            let empty = last.cells.iter().all(|c| match c {
                Cell::Text(e) => e.text().is_empty(),
                _ => true,
            });
            if empty && self.sel_row != self.rows.len() - 1 {
                self.rows.pop();
            } else {
                break;
            }
        }
    }

    pub fn select(&mut self, row: usize, col: usize) {
        self.sel_row = row.min(self.rows.len());
        self.sel_col = col.min(self.columns.len() - 1);
        self.drop_trailing_empty();
        if self.sel_row > self.rows.len() {
            self.sel_row = self.rows.len();
        }
    }

    fn move_row(&mut self, delta: isize) -> bool {
        let max = self.rows.len() as isize;
        let n = self.sel_row as isize + delta;
        if n < 0 || n > max {
            return false;
        }
        let col = self.sel_col;
        let target = n as usize;
        self.sel_row = target;
        self.drop_trailing_empty();
        self.sel_row = target.min(self.rows.len());
        // The trailing "add" row starts at its first column.
        self.sel_col = if self.sel_row == self.rows.len() {
            0
        } else {
            col
        };
        // Keep the text cursor column when moving between text cells.
        true
    }

    pub fn next_cell(&mut self) {
        if self.sel_col + 1 < self.columns.len() {
            self.sel_col += 1;
        } else {
            self.sel_col = 0;
            self.sel_row += 1;
            self.drop_trailing_empty();
            self.sel_row = self.sel_row.min(self.rows.len());
        }
        if let Some(Cell::Text(e)) = self.cell_mut() {
            e.move_to_end();
        }
    }

    pub fn toggle_row(&mut self) -> bool {
        if let Some(r) = self.rows.get_mut(self.sel_row) {
            r.enabled = !r.enabled;
            true
        } else {
            false
        }
    }

    pub fn delete_row(&mut self) -> bool {
        if self.sel_row < self.rows.len() {
            self.rows.remove(self.sel_row);
            self.sel_row = self.sel_row.min(self.rows.len());
            true
        } else {
            false
        }
    }

    /// Cycles a choice or flips a toggle in the selected cell.
    pub fn activate(&mut self, dir: isize) -> bool {
        let col = self.columns.get(self.sel_col).map(|c| c.kind);
        self.ensure_row();
        match (col, self.cell_mut()) {
            (Some(ColKind::Choice(opts)), Some(Cell::Choice(i))) => {
                let n = opts.len() as isize;
                *i = ((*i as isize + dir).rem_euclid(n)) as usize;
                true
            }
            (Some(ColKind::Toggle), Some(Cell::Toggle(b))) => {
                *b = !*b;
                true
            }
            _ => false,
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent, vim: bool, cb: &mut Clipboard) -> TableOutcome {
        let kind = self
            .columns
            .get(self.sel_col)
            .map(|c| c.kind)
            .unwrap_or(ColKind::Text);
        let no_mods = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Up if no_mods => {
                return if self.move_row(-1) {
                    TableOutcome::Handled
                } else {
                    TableOutcome::ExitUp
                };
            }
            KeyCode::Down if no_mods => {
                if self.sel_row >= self.rows.len() {
                    return TableOutcome::ExitDown;
                }
                return if self.move_row(1) {
                    TableOutcome::Handled
                } else {
                    TableOutcome::ExitDown
                };
            }
            _ => {}
        }
        if kind != ColKind::Text {
            return match key.code {
                KeyCode::Char(' ') => {
                    if self.activate(1) {
                        TableOutcome::Changed
                    } else {
                        TableOutcome::Handled
                    }
                }
                KeyCode::Right if no_mods => {
                    if self.sel_col + 1 < self.columns.len() {
                        self.sel_col += 1;
                    }
                    TableOutcome::Handled
                }
                KeyCode::Left if no_mods => {
                    if self.sel_col > 0 {
                        self.sel_col -= 1;
                    }
                    TableOutcome::Handled
                }
                KeyCode::Char('j') if vim && no_mods => {
                    if self.move_row(1) {
                        TableOutcome::Handled
                    } else {
                        TableOutcome::ExitDown
                    }
                }
                KeyCode::Char('k') if vim && no_mods => {
                    if self.move_row(-1) {
                        TableOutcome::Handled
                    } else {
                        TableOutcome::ExitUp
                    }
                }
                KeyCode::Esc => TableOutcome::Leave,
                _ => TableOutcome::Ignored,
            };
        }
        let at_new = self.sel_row >= self.rows.len();
        // Typing into the trailing row creates it.
        let typing = matches!(key.code, KeyCode::Char(_)) && no_mods;
        if at_new && !typing {
            return match key.code {
                KeyCode::Esc => TableOutcome::Leave,
                KeyCode::Right if no_mods && self.sel_col + 1 < self.columns.len() => {
                    self.sel_col += 1;
                    TableOutcome::Handled
                }
                KeyCode::Left if no_mods && self.sel_col > 0 => {
                    self.sel_col -= 1;
                    TableOutcome::Handled
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) && !vim => {
                    self.ensure_row();
                    self.forward(key, vim, cb)
                }
                _ => TableOutcome::Ignored,
            };
        }
        if at_new {
            if vim {
                // In vim mode the trailing row needs insert mode first.
                if let KeyCode::Char(c) = key.code {
                    if matches!(c, 'i' | 'a' | 'A' | 'I') {
                        self.ensure_row();
                        if let Some(e) = self.editor_mut() {
                            e.vim = crate::editor::VimMode::Insert;
                        }
                        return TableOutcome::Handled;
                    }
                    if c == 'j' {
                        return TableOutcome::ExitDown;
                    }
                    if c == 'k' {
                        return if self.move_row(-1) {
                            TableOutcome::Handled
                        } else {
                            TableOutcome::ExitUp
                        };
                    }
                    return TableOutcome::Handled;
                }
            }
            self.ensure_row();
        }
        self.forward(key, vim, cb)
    }

    fn forward(&mut self, key: &KeyEvent, vim: bool, cb: &mut Clipboard) -> TableOutcome {
        let ncols = self.columns.len();
        let col = self.sel_col;
        let Some(ed) = self.editor_mut() else {
            return TableOutcome::Ignored;
        };
        let at_start = ed.cursor.col == 0 && ed.anchor.is_none();
        let at_end = ed.cursor.col >= ed.text().chars().count() && ed.anchor.is_none();
        let in_insert = !vim || ed.vim == crate::editor::VimMode::Insert;
        // Left/Right at the edges of a cell move to the neighbouring cell.
        if key.modifiers.is_empty() && in_insert {
            if key.code == KeyCode::Right && at_end && col + 1 < ncols {
                self.sel_col += 1;
                if let Some(e) = self.editor_mut() {
                    e.cursor.col = 0;
                }
                return TableOutcome::Handled;
            }
            if key.code == KeyCode::Left && at_start && col > 0 {
                self.sel_col -= 1;
                if let Some(e) = self.editor_mut() {
                    e.move_to_end();
                }
                return TableOutcome::Handled;
            }
        }
        if vim
            && !in_insert
            && let KeyCode::Char('j') | KeyCode::Char('k') = key.code
        {
            let d = if key.code == KeyCode::Char('j') {
                1
            } else {
                -1
            };
            return if self.move_row(d) {
                TableOutcome::Handled
            } else if d > 0 {
                TableOutcome::ExitDown
            } else {
                TableOutcome::ExitUp
            };
        }
        let Some(ed) = self.editor_mut() else {
            return TableOutcome::Ignored;
        };
        match ed.handle_key(key, vim, cb) {
            Outcome::Changed => TableOutcome::Changed,
            Outcome::Handled => TableOutcome::Handled,
            Outcome::Submit => {
                self.next_cell();
                TableOutcome::Handled
            }
            Outcome::Leave => TableOutcome::Leave,
            Outcome::Ignored => TableOutcome::Ignored,
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.ensure_row();
        if let Some(e) = self.editor_mut() {
            e.insert_str(text);
        }
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.sel_row < self.scroll {
            self.scroll = self.sel_row;
        } else if self.sel_row >= self.scroll + height {
            self.scroll = self.sel_row + 1 - height;
        }
    }

    /// Whether the selected cell is a text editor in vim insert mode (or conventional).
    pub fn editing_text(&self) -> bool {
        matches!(
            self.columns.get(self.sel_col).map(|c| c.kind),
            Some(ColKind::Text)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_creates_rows_and_navigates() {
        let mut cb = Clipboard::default();
        let mut t = Table::kv("Name", "Value");
        assert_eq!(
            t.handle_key(&k(KeyCode::Up), false, &mut cb),
            TableOutcome::ExitUp
        );
        assert_eq!(
            t.handle_key(&k(KeyCode::Down), false, &mut cb),
            TableOutcome::ExitDown
        );
        for c in "X-A".chars() {
            t.handle_key(&k(KeyCode::Char(c)), false, &mut cb);
        }
        assert_eq!(t.rows.len(), 1);
        t.handle_key(&k(KeyCode::Enter), false, &mut cb);
        assert_eq!(t.sel_col, 1);
        for c in "1".chars() {
            t.handle_key(&k(KeyCode::Char(c)), false, &mut cb);
        }
        t.handle_key(&k(KeyCode::Enter), false, &mut cb);
        assert_eq!((t.sel_row, t.sel_col), (1, 0));
        assert_eq!(t.values().len(), 1);
        assert_eq!(t.rows[0].cells[0].text(), "X-A");
        assert_eq!(t.rows[0].cells[1].text(), "1");
        t.handle_key(&k(KeyCode::Up), false, &mut cb);
        assert!(t.toggle_row());
        assert!(!t.rows[0].enabled);
        t.handle_key(&k(KeyCode::Home), false, &mut cb);
        t.handle_key(&k(KeyCode::Left), false, &mut cb);
        assert_eq!(t.sel_col, 0);
        t.handle_key(&k(KeyCode::End), false, &mut cb);
        t.handle_key(&k(KeyCode::Right), false, &mut cb);
        assert_eq!(t.sel_col, 1);
        assert!(t.delete_row());
        assert!(t.rows.is_empty());
        assert!(!t.delete_row());
        assert!(!t.toggle_row());
        t.paste("pasted");
        assert_eq!(t.rows.len(), 1);
        t.ensure_visible(1);
    }

    #[test]
    fn choice_and_toggle_columns() {
        let mut cb = Clipboard::default();
        let mut t = Table::new(
            vec![
                Column {
                    title: "Var",
                    kind: ColKind::Text,
                    weight: 1,
                },
                Column {
                    title: "From",
                    kind: ColKind::Choice(&["a", "b", "c"]),
                    weight: 1,
                },
                Column {
                    title: "Secret",
                    kind: ColKind::Toggle,
                    weight: 1,
                },
            ],
            false,
        );
        t.select(0, 1);
        assert_eq!(
            t.handle_key(&k(KeyCode::Char(' ')), false, &mut cb),
            TableOutcome::Changed
        );
        assert_eq!(t.rows[0].cells[1].choice(), 1);
        assert!(t.activate(-1));
        assert_eq!(t.rows[0].cells[1].choice(), 0);
        t.handle_key(&k(KeyCode::Right), false, &mut cb);
        t.handle_key(&k(KeyCode::Char(' ')), false, &mut cb);
        assert!(t.rows[0].cells[2].toggle());
        t.handle_key(&k(KeyCode::Left), false, &mut cb);
        assert_eq!(t.sel_col, 1);
        assert_eq!(
            t.handle_key(&k(KeyCode::Esc), false, &mut cb),
            TableOutcome::Leave
        );
        assert!(!t.editing_text());
    }

    #[test]
    fn vim_rows() {
        let mut cb = Clipboard::default();
        let mut t = Table::kv("Name", "Value");
        t.push_texts(true, &["a", "1"]);
        t.push_texts(true, &["b", "2"]);
        t.select(0, 0);
        if let Some(e) = t.editor_mut() {
            e.vim = crate::editor::VimMode::Normal;
        }
        t.handle_key(&k(KeyCode::Char('j')), true, &mut cb);
        assert_eq!(t.sel_row, 1);
        t.handle_key(&k(KeyCode::Char('j')), true, &mut cb);
        assert_eq!(t.sel_row, 2);
        assert_eq!(
            t.handle_key(&k(KeyCode::Char('j')), true, &mut cb),
            TableOutcome::ExitDown
        );
        t.handle_key(&k(KeyCode::Char('i')), true, &mut cb);
        t.handle_key(&k(KeyCode::Char('c')), true, &mut cb);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[2].cells[0].text(), "c");
    }
}
