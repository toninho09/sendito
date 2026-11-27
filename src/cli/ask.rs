//! The values a `sendito run` asks for: `$prompt` and `$select`. The order
//! is: a `--prompt LABEL=VALUE` flag, then a question on the terminal, else
//! the placeholder stays unresolved.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};

use sendito_core::funcs::PreSend;
use sendito_core::model::Request;
use sendito_core::vars::{Rendered, Scope, VarValue};

/// The answers of one run, by label: a folder run asks each label once.
pub(crate) type Answers = BTreeMap<String, VarValue>;

/// Gives `scope` the values of the `$prompt` and `$select` calls of `req`.
/// A label with no flag asks on the terminal (only when `interactive`) and
/// keeps the answer in `answers`. The error names the labels that have no
/// value (no flag and no terminal), or a value that is not an option of its
/// `$select`.
pub(crate) fn answer_prompts(
    req: &Request,
    scope: &Scope,
    flags: &[(String, String)],
    answers: &mut Answers,
    interactive: bool,
) -> Result<(), String> {
    let mut missing = Vec::new();
    for (text, p) in sendito_core::resolve::pre_send_calls(req, scope) {
        let (label, secret, options) = match &p {
            PreSend::Prompt { label, secret } => (label, *secret, None),
            PreSend::Select { label, options } => (label, false, Some(options)),
            PreSend::Response { .. } => continue,
        };
        if !answers.contains_key(label) {
            let given = flags.iter().rev().find(|(l, _)| l == label);
            let value = match given {
                Some((_, v)) => Some(v.clone()),
                None if interactive
                    && std::io::stdin().is_terminal()
                    && std::io::stderr().is_terminal() =>
                {
                    Some(ask(label, secret, options).map_err(|e| format!("{label}: {e}"))?)
                }
                None => None,
            };
            let Some(value) = value else {
                if !missing.contains(label) {
                    missing.push(label.clone());
                }
                continue;
            };
            if let Some(o) = options
                && !o.contains(&value)
            {
                return Err(format!(
                    "{value:?} is not an option of {label:?} ({})",
                    o.join(", ")
                ));
            }
            answers.insert(label.clone(), VarValue { value, secret });
        }
        if let Some(v) = answers.get(label) {
            scope.keep(
                &text,
                Rendered {
                    text: v.value.clone(),
                    secret: v.secret,
                    unresolved: vec![],
                },
            );
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let flags: Vec<String> = missing
        .iter()
        .map(|l| format!("--prompt '{l}=…'"))
        .collect();
    Err(format!("no value to send for {}", flags.join(" ")))
}

/// Asks one value on the terminal: a line of text (not shown for a secret),
/// or the number of an option.
fn ask(label: &str, secret: bool, options: Option<&Vec<String>>) -> Result<String, String> {
    let mut err = std::io::stderr();
    if let Some(options) = options {
        for (i, o) in options.iter().enumerate() {
            let _ = writeln!(err, "  {}) {o}", i + 1);
        }
        let _ = write!(err, "{label} [1-{}]: ", options.len());
        let _ = err.flush();
        let line = read_line()?;
        let n: usize = line
            .trim()
            .parse()
            .map_err(|_| format!("{:?} is not a number", line.trim()))?;
        return options
            .get(n.wrapping_sub(1))
            .cloned()
            .ok_or_else(|| format!("{n} is not between 1 and {}", options.len()));
    }
    let _ = write!(err, "{label}: ");
    let _ = err.flush();
    if secret {
        let value = read_hidden();
        let _ = writeln!(err);
        return value;
    }
    read_line()
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Reads a line with the terminal echo off.
fn read_hidden() -> Result<String, String> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let mut value = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(value),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err("cancelled".to_string());
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Esc => break Err("cancelled".to_string()),
                _ => {}
            },
            Ok(Event::Paste(s)) => value.push_str(&s),
            Ok(_) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    result
}
