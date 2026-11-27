//! The state of a run tab: a folder run of the shared suite engine
//! (`sendito_pipeline::suite`), one row per finished request.

use std::time::{Duration, Instant};

use sendito_pipeline::suite::{Cancel, Entry, Row};

use super::Response;

/// One finished request of a run.
pub struct RunRow {
    pub entry: Entry,
    /// Its response pane. `None` while the tab shows it (the pane owns it
    /// then, so its scroll and view stay when the selection moves back).
    pub response: Option<Response>,
}

/// A folder run shown in a tab.
pub struct RunView {
    /// The folder (`""`: the collection root).
    pub folder: String,
    /// The data file as typed, when there is one.
    pub data_file: Option<String>,
    /// The requests, in run order.
    pub rels: Vec<String>,
    /// The data rows; empty without a data file.
    pub data: Vec<Row>,
    pub rows: Vec<RunRow>,
    pub sel: usize,
    pub scroll: usize,
    /// The row whose response the tab shows.
    pub shown: Option<usize>,
    /// Lines of the request in flight (notes, retries, session progress);
    /// they go to its row's Messages when it finishes.
    pub pending_log: Vec<String>,
    pub started: Instant,
    /// How long the last run took; `None` while it runs.
    pub took: Option<Duration>,
    /// Stops the run in flight.
    pub cancel: Cancel,
}

impl RunView {
    pub fn new(folder: &str, data_file: Option<String>, rels: Vec<String>, data: Vec<Row>) -> Self {
        Self {
            folder: folder.to_string(),
            data_file,
            rels,
            data,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            shown: None,
            pending_log: Vec::new(),
            started: Instant::now(),
            took: None,
            cancel: Cancel::default(),
        }
    }

    /// How many sends the run makes.
    pub fn planned(&self) -> usize {
        self.rels.len() * self.data.len().max(1)
    }

    /// The rows with `outcome`.
    pub fn count(&self, outcome: &str) -> usize {
        self.rows
            .iter()
            .filter(|r| r.entry.outcome == outcome)
            .count()
    }

    /// Clears the rows for a new run.
    pub fn restart(&mut self) {
        self.rows.clear();
        self.sel = 0;
        self.scroll = 0;
        self.shown = None;
        self.pending_log.clear();
        self.started = Instant::now();
        self.took = None;
        self.cancel = Cancel::default();
    }
}
