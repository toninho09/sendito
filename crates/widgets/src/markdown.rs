//! A read-only Markdown preview: CommonMark text to styled terminal lines,
//! wrapped at a width. Headings, emphasis, code, lists, task lists, quotes,
//! links, rules and tables are drawn; HTML shows as plain text.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

/// A block that indents the lines inside it.
enum Container {
    Quote,
    /// A list item; the value is the width of its marker.
    Item(usize),
}

struct Renderer<'t> {
    theme: &'t Theme,
    width: usize,
    lines: Vec<Line<'static>>,
    /// The text of the current paragraph, heading or table row.
    runs: Vec<(String, Style)>,
    styles: Vec<Style>,
    containers: Vec<Container>,
    /// The next number of each open list; `None` for a bullet list.
    lists: Vec<Option<u64>>,
    /// The marker of a list item whose first line is not written yet.
    marker: Option<String>,
    in_code: bool,
    links: Vec<String>,
}

/// Renders `text` as lines no wider than `width` columns.
pub fn render(text: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let mut r = Renderer {
        theme,
        width: usize::from(width.max(8)),
        lines: Vec::new(),
        runs: Vec::new(),
        styles: vec![theme.fg(theme.fg)],
        containers: Vec::new(),
        lists: Vec::new(),
        marker: None,
        in_code: false,
        links: Vec::new(),
    };
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text, options) {
        r.event(event);
    }
    r.flush();
    while r.lines.last().is_some_and(|l| l.width() == 0) {
        r.lines.pop();
    }
    r.lines
}

impl Renderer<'_> {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn code_style(&self) -> Style {
        self.theme.fg(self.theme.json_str)
    }

    fn text(&mut self, t: &str) {
        let style = self.style();
        self.runs.push((t.to_string(), style));
    }

    /// The indent of the lines inside the open containers. `first` uses the
    /// pending list marker in place of the item's indent.
    fn prefix(&mut self, first: bool) -> String {
        let mut out = String::new();
        let last = self.containers.len().saturating_sub(1);
        for (i, c) in self.containers.iter().enumerate() {
            match c {
                Container::Quote => out.push_str("│ "),
                Container::Item(w) => match self.marker.as_ref() {
                    Some(m) if first && i == last => out.push_str(m),
                    _ => out.push_str(&" ".repeat(*w)),
                },
            }
        }
        if first {
            self.marker = None;
        }
        out
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| l.width() > 0) {
            self.lines.push(Line::default());
        }
    }

    /// Wraps the collected runs into lines.
    fn flush(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let first = self.prefix(true);
        let rest = self.prefix(false);
        let dim = self.theme.dim();
        let mut line: Vec<Span<'static>> = vec![Span::styled(first.clone(), dim)];
        let mut used = width(&first);
        let mut lines = Vec::new();
        for (text, style) in runs {
            for word in split_words(&text) {
                if word == "\n" {
                    lines.push(Line::from(std::mem::take(&mut line)));
                    line.push(Span::styled(rest.clone(), dim));
                    used = width(&rest);
                    continue;
                }
                let at_start = used == width(&rest) || line.len() == 1;
                if word.trim().is_empty() {
                    if !at_start && used < self.width {
                        line.push(Span::styled(word.to_string(), style));
                        used += width(word);
                    }
                    continue;
                }
                let w = width(word);
                if used + w > self.width && !at_start {
                    trim_end(&mut line);
                    lines.push(Line::from(std::mem::take(&mut line)));
                    line.push(Span::styled(rest.clone(), dim));
                    used = width(&rest);
                }
                // A word longer than the line is cut where the line ends.
                let mut part = String::new();
                for c in word.chars() {
                    let cw = c.width().unwrap_or(0);
                    if used + cw > self.width && used > width(&rest) {
                        line.push(Span::styled(std::mem::take(&mut part), style));
                        lines.push(Line::from(std::mem::take(&mut line)));
                        line.push(Span::styled(rest.clone(), dim));
                        used = width(&rest);
                    }
                    part.push(c);
                    used += cw;
                }
                line.push(Span::styled(part, style));
            }
        }
        trim_end(&mut line);
        lines.push(Line::from(line));
        self.lines.extend(lines);
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) if self.in_code => {
                let style = self.code_style();
                for l in t.lines() {
                    let p = self.prefix(true);
                    self.lines.push(Line::from(vec![
                        Span::styled(p, self.theme.dim()),
                        Span::styled(format!("  {l}"), style),
                    ]));
                }
            }
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                let style = self.code_style();
                self.runs.push((t.to_string(), style));
            }
            Event::Html(t) | Event::InlineHtml(t) => self.text(&t),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t),
            Event::FootnoteReference(t) => self.text(&format!("[{t}]")),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.runs.push(("\n".into(), Style::default())),
            Event::Rule => {
                self.flush();
                let p = self.prefix(true);
                let n = self.width.saturating_sub(width(&p));
                self.lines.push(Line::from(vec![
                    Span::styled(p, self.theme.dim()),
                    Span::styled("─".repeat(n), self.theme.dim()),
                ]));
                self.blank();
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                let accent = self.theme.fg(self.theme.accent);
                self.styles.push(accent.add_modifier(
                    if level == pulldown_cmark::HeadingLevel::H1 {
                        Modifier::BOLD | Modifier::UNDERLINED
                    } else {
                        Modifier::BOLD
                    },
                ));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.containers.push(Container::Quote);
                self.push_style(|s| s.add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.in_code = true;
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    let p = self.prefix(true);
                    self.lines.push(Line::from(vec![
                        Span::styled(p, self.theme.dim()),
                        Span::styled(format!("  {lang}"), self.theme.dim()),
                    ]));
                }
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_string(),
                };
                self.containers.push(Container::Item(width(&marker)));
                self.marker = Some(marker);
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                let info = self.theme.fg(self.theme.info);
                self.styles.push(info.add_modifier(Modifier::UNDERLINED));
                self.links.push(dest_url.to_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank();
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.pop_style();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.pop_style();
                self.blank();
            }
            TagEnd::CodeBlock => {
                self.in_code = false;
                self.blank();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.flush();
                // A loose item ends with a blank line; the next item needs none.
                while self.lines.last().is_some_and(|l| l.width() == 0) {
                    self.lines.pop();
                }
                self.containers.pop();
                self.marker = None;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link | TagEnd::Image => {
                self.pop_style();
                if let Some(url) = self.links.pop() {
                    let shown = self.runs.last().is_some_and(|(t, _)| *t == url);
                    if !url.is_empty() && !shown {
                        let dim = self.theme.dim();
                        self.runs.push((format!(" ({url})"), dim));
                    }
                }
            }
            TagEnd::TableCell => {
                let dim = self.theme.dim();
                self.runs.push((" │ ".into(), dim));
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some((t, _)) = self.runs.last()
                    && t == " │ "
                {
                    self.runs.pop();
                }
                self.flush();
            }
            TagEnd::Table => self.blank(),
            _ => {}
        }
    }
}

fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Words and the runs of spaces between them; a newline is its own item.
fn split_words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            if start < i {
                out.push(&text[start..i]);
            }
            out.push("\n");
            start = i + 1;
            space = None;
            continue;
        }
        let is_space = c == ' ';
        if space.is_some_and(|s| s != is_space) && start < i {
            out.push(&text[start..i]);
            start = i;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Removes the spaces at the end of a line.
fn trim_end(line: &mut Vec<Span<'static>>) {
    while line.len() > 1 && line.last().is_some_and(|s| s.content.trim().is_empty()) {
        line.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str, width: u16) -> Vec<String> {
        render(text, width, &Theme::by_name("dusk"))
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn headings_paragraphs_and_emphasis() {
        let lines = render(
            "# Users\n\nAll **user** calls.",
            40,
            &Theme::by_name("dusk"),
        );
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content == "Users" && s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            lines[2]
                .spans
                .iter()
                .any(|s| s.content == "user" && s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert_eq!(
            plain("# Users\n\nAll **user** calls.", 40)[2],
            "All user calls."
        );
    }

    #[test]
    fn lists_quotes_code_and_links() {
        assert_eq!(
            plain(
                "- one\n- two\n  1. a\n  2. b\n\n> note\n\n```json\n{}\n```\n\n[docs](https://x.dev) `id`",
                40
            ),
            [
                "• one",
                "• two",
                "  1. a",
                "  2. b",
                "",
                "│ note",
                "",
                "  json",
                "  {}",
                "",
                "docs (https://x.dev) id",
            ]
        );
        assert_eq!(
            plain("- [x] done\n- [ ] open", 40),
            ["• [x] done", "• [ ] open"]
        );
        assert_eq!(plain("<https://x.dev>", 40), ["https://x.dev"]);
    }

    #[test]
    fn wraps_at_the_width_and_keeps_the_indent() {
        assert_eq!(
            plain("- one two three four five", 12),
            ["• one two", "  three four", "  five"]
        );
        assert_eq!(plain("abcdefghijkl", 8), ["abcdefgh", "ijkl"]);
        assert_eq!(plain("a  \nb", 20), ["a", "b"]);
        assert_eq!(plain("---", 8), ["────────"]);
        assert_eq!(
            plain("| a | b |\n|---|---|\n| 1 | 2 |", 20),
            ["a │ b", "1 │ 2"]
        );
        assert!(plain("", 20).is_empty());
    }
}
