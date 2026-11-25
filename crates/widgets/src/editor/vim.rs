//! The vim subset: motions, operators, normal, visual and visual-line mode.

use super::*;

impl TextEditor {
    pub(super) fn line_first_nonblank(&self, row: usize) -> usize {
        self.lines[row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count()
    }

    /// Resolves a vim motion key. Returns (target, linewise, inclusive).
    pub(super) fn motion(&mut self, keys: &[char]) -> Option<(Pos, bool, bool)> {
        let c = self.cursor;
        let last_col = |r: usize| char_len(&self.lines[r]).saturating_sub(1);
        Some(match keys {
            ['h'] => (
                Pos {
                    col: c.col.saturating_sub(1),
                    ..c
                },
                false,
                false,
            ),
            ['l'] => (
                Pos {
                    col: (c.col + 1).min(char_len(&self.lines[c.row])),
                    ..c
                },
                false,
                false,
            ),
            ['j'] if !self.single_line => (
                Pos {
                    row: (c.row + 1).min(self.lines.len() - 1),
                    col: c.col,
                },
                true,
                true,
            ),
            ['k'] if !self.single_line => (
                Pos {
                    row: c.row.saturating_sub(1),
                    col: c.col,
                },
                true,
                true,
            ),
            ['w'] => (self.word_forward(c, false), false, false),
            ['e'] => (self.word_forward(c, true), false, true),
            ['b'] => (self.word_backward(c), false, false),
            ['0'] => (Pos { col: 0, ..c }, false, false),
            ['^'] => (
                Pos {
                    col: self.line_first_nonblank(c.row),
                    ..c
                },
                false,
                false,
            ),
            ['$'] => (
                Pos {
                    col: last_col(c.row),
                    ..c
                },
                false,
                true,
            ),
            ['g', 'g'] => (Pos { row: 0, col: 0 }, true, true),
            ['G'] => (
                Pos {
                    row: self.lines.len() - 1,
                    col: 0,
                },
                true,
                true,
            ),
            _ => return None,
        })
    }

    pub(super) fn is_motion_prefix(keys: &[char]) -> bool {
        matches!(keys, ['g'])
    }

    pub(super) fn normal_clamp(&mut self) {
        let len = char_len(&self.lines[self.cursor.row]);
        if self.vim == VimMode::Normal && len > 0 && self.cursor.col >= len {
            self.cursor.col = len - 1;
        }
    }

    pub(super) fn enter_insert(&mut self) {
        self.vim = VimMode::Insert;
        self.anchor = None;
        self.snapshot();
        self.typing = true;
    }

    pub(super) fn yank_lines(&self, s: usize, e: usize, cb: &mut Clipboard) {
        let mut t = self.lines[s..=e].join("\n");
        t.push('\n');
        cb.set(t, true);
    }

    pub(super) fn delete_lines(&mut self, s: usize, e: usize) {
        self.lines.drain(s..=e);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.row = s.min(self.lines.len() - 1);
        self.cursor.col = self.line_first_nonblank(self.cursor.row);
    }

    pub(super) fn apply_operator(
        &mut self,
        op: char,
        target: Pos,
        linewise: bool,
        inclusive: bool,
        cb: &mut Clipboard,
    ) -> Outcome {
        if linewise {
            let (s, e) = if target.row < self.cursor.row {
                (target.row, self.cursor.row)
            } else {
                (self.cursor.row, target.row)
            };
            self.yank_lines(s, e, cb);
            if op == 'y' {
                return Outcome::Handled;
            }
            if self.read_only {
                return Outcome::Handled;
            }
            self.snapshot();
            if op == 'c' {
                self.lines.drain(s + 1..=e);
                self.lines[s].clear();
                self.cursor = Pos { row: s, col: 0 };
                self.vim = VimMode::Insert;
                self.typing = true;
            } else {
                self.delete_lines(s, e);
            }
            return Outcome::Changed;
        }
        let (s, mut e) = if target < self.cursor {
            (target, self.cursor)
        } else {
            (self.cursor, target)
        };
        if inclusive {
            e.col = (e.col + 1).min(char_len(&self.lines[e.row]));
        }
        let text = self.range_text(s, e);
        cb.set(text, false);
        if op == 'y' {
            self.cursor = s;
            return Outcome::Handled;
        }
        if self.read_only {
            return Outcome::Handled;
        }
        self.snapshot();
        self.delete_range(s, e);
        if op == 'c' {
            self.vim = VimMode::Insert;
            self.typing = true;
        } else {
            self.normal_clamp();
        }
        Outcome::Changed
    }

    pub(super) fn paste_vim(&mut self, before: bool, cb: &Clipboard) -> Outcome {
        if self.read_only || cb.text.is_empty() {
            return Outcome::Handled;
        }
        if cb.linewise && !self.single_line {
            self.snapshot();
            let text = cb.text.strip_suffix('\n').unwrap_or(&cb.text);
            let at = if before {
                self.cursor.row
            } else {
                self.cursor.row + 1
            };
            for (i, l) in text.split('\n').enumerate() {
                self.lines.insert(at + i, l.to_string());
            }
            self.cursor = Pos {
                row: at,
                col: self.line_first_nonblank(at),
            };
            return Outcome::Changed;
        }
        let text = cb.text.clone();
        if !before && !self.lines[self.cursor.row].is_empty() {
            self.cursor.col += 1;
        }
        self.insert_str(&text);
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
        Outcome::Changed
    }

    pub(super) fn handle_vim(&mut self, key: &KeyEvent, cb: &mut Clipboard) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Arrow keys and friends behave as motions.
        let code = match key.code {
            KeyCode::Left => KeyCode::Char('h'),
            KeyCode::Right => KeyCode::Char('l'),
            KeyCode::Up => KeyCode::Char('k'),
            KeyCode::Down => KeyCode::Char('j'),
            KeyCode::Home => KeyCode::Char('0'),
            KeyCode::End => KeyCode::Char('$'),
            other => other,
        };
        if matches!(key.code, KeyCode::Up | KeyCode::Down) && self.single_line {
            return Outcome::Ignored;
        }
        match code {
            KeyCode::Esc => {
                if !self.pending.is_empty() {
                    self.pending.clear();
                    return Outcome::Handled;
                }
                if matches!(self.vim, VimMode::Visual | VimMode::VisualLine) {
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    return Outcome::Handled;
                }
                Outcome::Leave
            }
            KeyCode::Enter => {
                if self.single_line {
                    Outcome::Submit
                } else {
                    self.vertical(1, false);
                    Outcome::Handled
                }
            }
            KeyCode::Char('r') if ctrl => {
                if self.redo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char(_) if ctrl => Outcome::Ignored,
            KeyCode::Char(c) => {
                self.pending.push(c);
                let keys = self.pending.clone();
                let out = self.vim_keys(&keys, cb);
                match out {
                    None => Outcome::Handled,
                    Some(o) => {
                        self.pending.clear();
                        self.normal_clamp();
                        o
                    }
                }
            }
            KeyCode::Backspace => {
                let to = if self.cursor.col > 0 {
                    Pos {
                        col: self.cursor.col - 1,
                        ..self.cursor
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, false);
                Outcome::Handled
            }
            _ => Outcome::Ignored,
        }
    }

    /// Returns `None` while a multi-key command is incomplete.
    pub(super) fn vim_keys(&mut self, keys: &[char], cb: &mut Clipboard) -> Option<Outcome> {
        let visual = matches!(self.vim, VimMode::Visual | VimMode::VisualLine);
        if visual {
            match keys {
                ['d'] | ['x'] | ['c'] | ['y'] => {
                    let op = if keys[0] == 'x' { 'd' } else { keys[0] };
                    let linewise = self.vim == VimMode::VisualLine;
                    let sel = self.selection();
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    let Some((s, e)) = sel else {
                        return Some(Outcome::Handled);
                    };
                    if linewise {
                        self.cursor = s;
                        return Some(self.apply_operator(op, e, true, true, cb));
                    }
                    let text = self.range_text(s, e);
                    cb.set(text, false);
                    if op == 'y' || self.read_only {
                        self.cursor = s;
                        return Some(Outcome::Handled);
                    }
                    self.snapshot();
                    self.delete_range(s, e);
                    if op == 'c' {
                        self.vim = VimMode::Insert;
                        self.typing = true;
                    }
                    return Some(Outcome::Changed);
                }
                ['v'] => {
                    self.vim = if self.vim == VimMode::Visual {
                        VimMode::Normal
                    } else {
                        VimMode::Visual
                    };
                    if self.vim == VimMode::Normal {
                        self.anchor = None;
                    }
                    return Some(Outcome::Handled);
                }
                ['V'] => {
                    self.vim = if self.vim == VimMode::VisualLine {
                        VimMode::Normal
                    } else {
                        VimMode::VisualLine
                    };
                    if self.vim == VimMode::Normal {
                        self.anchor = None;
                    }
                    return Some(Outcome::Handled);
                }
                _ => {}
            }
            if Self::is_motion_prefix(keys) {
                return None;
            }
            if let Some((to, _, _)) = self.motion(keys) {
                self.cursor = to;
                self.clamp();
                return Some(Outcome::Handled);
            }
            return Some(Outcome::Handled);
        }
        let ro = self.read_only;
        match keys {
            ['i'] if !ro => {
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['a'] if !ro => {
                if !self.lines[self.cursor.row].is_empty() {
                    self.cursor.col += 1;
                }
                self.enter_insert();
                self.clamp();
                Some(Outcome::Handled)
            }
            ['I'] if !ro => {
                self.cursor.col = self.line_first_nonblank(self.cursor.row);
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['A'] if !ro => {
                self.cursor.col = char_len(&self.lines[self.cursor.row]);
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['o'] | ['O'] if !ro && !self.single_line => {
                self.snapshot();
                let indent: String = self.lines[self.cursor.row]
                    .chars()
                    .take_while(|c| *c == ' ')
                    .collect();
                let at = if keys[0] == 'o' {
                    self.cursor.row + 1
                } else {
                    self.cursor.row
                };
                self.lines.insert(at, indent.clone());
                self.cursor = Pos {
                    row: at,
                    col: char_len(&indent),
                };
                self.vim = VimMode::Insert;
                self.typing = true;
                Some(Outcome::Changed)
            }
            ['v'] => {
                self.vim = VimMode::Visual;
                self.anchor = Some(self.cursor);
                Some(Outcome::Handled)
            }
            ['V'] => {
                self.vim = VimMode::VisualLine;
                self.anchor = Some(self.cursor);
                Some(Outcome::Handled)
            }
            ['x'] if !ro => {
                if self.lines[self.cursor.row].is_empty() {
                    return Some(Outcome::Handled);
                }
                let c = self.cursor;
                Some(self.apply_operator('d', c, false, true, cb))
            }
            ['X'] if !ro => {
                if self.cursor.col == 0 {
                    return Some(Outcome::Handled);
                }
                let to = Pos {
                    col: self.cursor.col - 1,
                    ..self.cursor
                };
                Some(self.apply_operator('d', to, false, false, cb))
            }
            ['D'] | ['C'] if !ro => {
                let len = char_len(&self.lines[self.cursor.row]);
                let op = if keys[0] == 'D' { 'd' } else { 'c' };
                if len == 0 {
                    if op == 'c' {
                        self.enter_insert();
                    }
                    return Some(Outcome::Handled);
                }
                let to = Pos {
                    col: len - 1,
                    ..self.cursor
                };
                Some(self.apply_operator(op, to, false, true, cb))
            }
            ['p'] => Some(self.paste_vim(false, cb)),
            ['P'] => Some(self.paste_vim(true, cb)),
            ['u'] => Some(if self.undo() {
                Outcome::Changed
            } else {
                Outcome::Handled
            }),
            ['d'] | ['c'] | ['y'] | ['g'] | ['z'] => None,
            ['d', 'd'] | ['c', 'c'] | ['y', 'y'] => {
                let row = self.cursor.row;
                Some(self.apply_operator(keys[0], Pos { row, col: 0 }, true, true, cb))
            }
            [op @ ('d' | 'c' | 'y'), rest @ ..] => {
                if Self::is_motion_prefix(rest) {
                    return None;
                }
                // `cw` changes to the end of the word, like vim.
                let rest: Vec<char> = if *op == 'c' && rest == ['w'] {
                    vec!['e']
                } else {
                    rest.to_vec()
                };
                match self.motion(&rest) {
                    Some((to, lw, inc)) => Some(self.apply_operator(*op, to, lw, inc, cb)),
                    None => Some(Outcome::Handled),
                }
            }
            _ => {
                if let Some((to, _, _)) = self.motion(keys) {
                    let to = if matches!(keys, ['G'] | ['g', 'g']) {
                        Pos {
                            col: self.line_first_nonblank(to.row),
                            ..to
                        }
                    } else {
                        to
                    };
                    if matches!(keys, ['j'] | ['k']) {
                        let want = self.preferred_col.unwrap_or(self.cursor.col);
                        self.cursor = Pos {
                            row: to.row,
                            col: want.min(char_len(&self.lines[to.row])),
                        };
                        self.preferred_col = Some(want);
                    } else {
                        self.cursor = to;
                        self.preferred_col = None;
                    }
                    self.clamp();
                    return Some(Outcome::Handled);
                }
                if matches!(keys, ['j'] | ['k']) {
                    return Some(Outcome::Ignored);
                }
                Some(Outcome::Handled)
            }
        }
    }
}
