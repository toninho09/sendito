//! Editing keys without vim: arrows, words, selection and clipboard.

use super::*;

impl TextEditor {
    pub(super) fn handle_conventional(
        &mut self,
        key: &KeyEvent,
        vim: bool,
        cb: &mut Clipboard,
    ) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                if vim {
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    self.typing = false;
                    if self.cursor.col > 0 {
                        self.cursor.col -= 1;
                    }
                    return Outcome::Handled;
                }
                if self.anchor.is_some() {
                    self.anchor = None;
                    return Outcome::Handled;
                }
                Outcome::Leave
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.type_char(c);
                Outcome::Changed
            }
            KeyCode::Char('z') if ctrl && !vim => {
                if self.undo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char('y') if ctrl && !vim => {
                if self.redo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char('a') if ctrl && !vim => {
                self.select_all();
                Outcome::Handled
            }
            KeyCode::Char('c') if ctrl && !vim => {
                self.copy(cb);
                Outcome::Handled
            }
            KeyCode::Char('x') if ctrl && !vim => {
                if self.read_only {
                    self.copy(cb);
                    return Outcome::Handled;
                }
                if self.selection().is_none() {
                    self.select_all();
                }
                self.copy(cb);
                self.snapshot();
                self.delete_selection();
                Outcome::Changed
            }
            KeyCode::Char('v') if ctrl && !vim => {
                if self.read_only || cb.text.is_empty() {
                    return Outcome::Handled;
                }
                let t = cb.text.clone();
                self.insert_str(&t);
                Outcome::Changed
            }
            KeyCode::Char('w') if ctrl && vim => Outcome::Ignored,
            KeyCode::Enter if !ctrl && !alt => {
                if self.single_line {
                    return Outcome::Submit;
                }
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.newline();
                Outcome::Changed
            }
            KeyCode::Tab if !self.single_line && !shift && !ctrl => {
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.insert_str("  ");
                Outcome::Changed
            }
            KeyCode::Backspace => {
                if self.backspace() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Delete => {
                if self.delete_forward() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Left => {
                if self.anchor.is_some() && !shift && self.selection().is_some() {
                    let (s, _) = self.selection().unwrap_or_default();
                    self.move_cursor(s, false);
                    return Outcome::Handled;
                }
                let to = if ctrl || alt {
                    self.word_backward(self.cursor)
                } else if self.cursor.col > 0 {
                    Pos {
                        col: self.cursor.col - 1,
                        ..self.cursor
                    }
                } else if self.cursor.row > 0 {
                    Pos {
                        row: self.cursor.row - 1,
                        col: char_len(&self.lines[self.cursor.row - 1]),
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::Right => {
                if self.anchor.is_some() && !shift && self.selection().is_some() {
                    let (_, e) = self.selection().unwrap_or_default();
                    self.move_cursor(e, false);
                    return Outcome::Handled;
                }
                let len = char_len(&self.lines[self.cursor.row]);
                let to = if ctrl || alt {
                    self.word_forward(self.cursor, false)
                } else if self.cursor.col < len {
                    Pos {
                        col: self.cursor.col + 1,
                        ..self.cursor
                    }
                } else if self.cursor.row + 1 < self.lines.len() {
                    Pos {
                        row: self.cursor.row + 1,
                        col: 0,
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::Up => {
                if self.single_line {
                    return Outcome::Ignored;
                }
                if self.vertical(-1, shift) {
                    Outcome::Handled
                } else {
                    Outcome::Ignored
                }
            }
            KeyCode::Down => {
                if self.single_line {
                    return Outcome::Ignored;
                }
                if self.vertical(1, shift) {
                    Outcome::Handled
                } else {
                    Outcome::Ignored
                }
            }
            KeyCode::PageUp if !self.single_line => {
                let h = self.last_size.1.max(2) as isize - 1;
                self.vertical(-h, shift);
                Outcome::Handled
            }
            KeyCode::PageDown if !self.single_line => {
                let h = self.last_size.1.max(2) as isize - 1;
                self.vertical(h, shift);
                Outcome::Handled
            }
            KeyCode::Home => {
                let to = if ctrl {
                    Pos::default()
                } else {
                    Pos {
                        col: 0,
                        ..self.cursor
                    }
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::End => {
                let to = if ctrl {
                    let row = self.lines.len() - 1;
                    Pos {
                        row,
                        col: char_len(&self.lines[row]),
                    }
                } else {
                    Pos {
                        col: char_len(&self.lines[self.cursor.row]),
                        ..self.cursor
                    }
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            _ => Outcome::Ignored,
        }
    }
}
