//! The send pipeline shared by the TUI and `sendito run` (scope → pre-request
//! script → resolve → send → extract/assert → history) and the response
//! document model it produces.

pub mod doc;
pub mod runner;

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}
