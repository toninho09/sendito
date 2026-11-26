//! The values a send asks for: `$prompt` and `$select` (`FORMAT.md`,
//! "Functions"). Each label is one question; the questions open one after the
//! other, then the send starts again with the answers.

use sendito_core::funcs::PreSend;
use sendito_core::vars::Rendered;

use super::*;

/// The answers of one send, by label.
pub type Answers = BTreeMap<String, VarValue>;

/// The questions of one send, while they are asked.
pub struct Asks {
    tab: u64,
    from_watch: bool,
    /// The questions not asked yet, in order.
    pending: Vec<(String, PreSend)>,
    /// The question on screen: its label and whether the value is a secret.
    current: Option<(String, bool)>,
    total: usize,
    answers: Answers,
}

/// The label of a `$prompt` or `$select`; `None` for another call.
fn label(p: &PreSend) -> Option<&str> {
    match p {
        PreSend::Prompt { label, .. } | PreSend::Select { label, .. } => Some(label),
        PreSend::Response { .. } => None,
    }
}

impl App {
    /// The values the `$prompt` and `$select` calls of `req` get, by call
    /// text. `None` when a question opened: the send stops and starts again
    /// when the last question is answered.
    pub(super) fn asked_values(
        &mut self,
        tab: u64,
        req: &Request,
        names: &Scope,
        from_watch: bool,
    ) -> Option<Vec<(String, Rendered)>> {
        let calls: Vec<(String, PreSend)> = sendito_core::resolve::pre_send_calls(req, names)
            .into_iter()
            .filter(|(_, p)| label(p).is_some())
            .collect();
        if calls.is_empty() {
            return Some(Vec::new());
        }
        let answers = if from_watch {
            self.tab_by_id(tab)
                .and_then(|t| t.watch.as_ref())
                .map(|w| w.answers.clone())
        } else {
            self.answers
                .take()
                .filter(|(t, _)| *t == tab)
                .map(|(_, a)| a)
        }
        .unwrap_or_default();
        let missing = |p: &PreSend| label(p).is_some_and(|l| !answers.contains_key(l));
        if !calls.iter().any(|(_, p)| missing(p)) {
            if let Some(w) = self.tab_by_id(tab).and_then(|t| t.watch.as_mut()) {
                w.answers = answers.clone();
            }
            let kept = calls
                .iter()
                .filter_map(|(text, p)| {
                    let v = answers.get(label(p)?)?;
                    Some((
                        text.clone(),
                        Rendered {
                            text: v.value.clone(),
                            secret: v.secret,
                            unresolved: vec![],
                        },
                    ))
                })
                .collect();
            return Some(kept);
        }
        let mut pending: Vec<(String, PreSend)> = Vec::new();
        for (_, p) in calls {
            if let Some(l) = label(&p)
                && !pending.iter().any(|(q, _)| q == l)
            {
                pending.push((l.to_string(), p));
            }
        }
        self.asks = Some(Asks {
            tab,
            from_watch,
            total: pending.len(),
            pending,
            current: None,
            answers: Answers::new(),
        });
        self.ask_next();
        None
    }

    /// Opens the next question, or sends again when every one has an answer.
    fn ask_next(&mut self) {
        let Some(asks) = self.asks.as_mut() else {
            return;
        };
        if asks.pending.is_empty() {
            let Some(asks) = self.asks.take() else {
                return;
            };
            if self.tab().is_some_and(|t| t.id == asks.tab) {
                self.answers = Some((asks.tab, asks.answers));
                self.send_opts(true, asks.from_watch);
            } else {
                self.set_status("not sent: the tab changed", StatusKind::Warn);
            }
            return;
        }
        let (label, p) = asks.pending.remove(0);
        let step = asks.total - asks.pending.len();
        let of = if asks.total > 1 {
            format!(" ({step} of {})", asks.total)
        } else {
            String::new()
        };
        let last = self.prompt_values.get(&label).cloned();
        match p {
            PreSend::Select { options, .. } => {
                asks.current = Some((label.clone(), false));
                let current = last.and_then(|v| options.iter().position(|o| *o == v));
                self.overlay = Some(Overlay::Picker(Picker::new(
                    &format!("{label}: value to send{of}"),
                    options,
                    vec![],
                    PickAction::SendValue,
                    current,
                )));
            }
            PreSend::Prompt { secret, .. } => {
                asks.current = Some((label.clone(), secret));
                let mut input = TextEditor::single(&if secret {
                    String::new()
                } else {
                    last.unwrap_or_default()
                });
                input.mask = secret;
                self.overlay = Some(Overlay::Prompt(Prompt::new(
                    &format!("Value to send{of}"),
                    &label,
                    input,
                    vec![],
                    PromptAction::SendValue,
                )));
            }
            PreSend::Response { .. } => self.ask_next(),
        }
    }

    /// Keeps the answer of the question on screen, then asks the next one.
    pub(super) fn answer(&mut self, value: String) {
        let Some((label, secret)) = self.asks.as_mut().and_then(|a| a.current.take()) else {
            return;
        };
        if !secret && self.prompt_values.get(&label) != Some(&value) {
            self.prompt_values.insert(label.clone(), value.clone());
            self.session_dirty = true;
        }
        if let Some(a) = self.asks.as_mut() {
            a.answers.insert(label, VarValue { value, secret });
        }
        self.ask_next();
    }
}
