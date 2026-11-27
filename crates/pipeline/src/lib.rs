//! The send pipeline shared by the TUI and `sendito run` (scope → pre-request
//! script → resolve → send → extract/assert → history) and the response
//! document model it produces.

pub mod diagnose;
pub mod doc;
pub mod pretty;
pub mod runner;
pub mod suite;

/// The protocol types that callers of the pipeline (the TUI) read.
pub mod http {
    pub use sendito_http::{HttpResponse, ProxySpec, Timing, guess_mime};
    pub use sendito_http::{graphql, sse};
}

/// The gRPC types that callers of the pipeline read.
pub mod grpc {
    pub use sendito_grpc::{CallKind, Event, MethodInfo, ServiceInfo, code_name};
}

/// The WebSocket types that callers of the pipeline read.
pub mod ws {
    pub use sendito_ws::{Event, FrameKind};
}

/// The MCP types and calls that callers of the pipeline use.
pub mod mcp {
    pub use sendito_mcp::oauth::login_http;
    pub use sendito_mcp::{
        Catalog, McpError, Options, Pool, Prompt, Resource, Tool, list_catalog, list_tools,
    };
}

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}
