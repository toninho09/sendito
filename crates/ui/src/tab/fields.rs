//! The request fields a tab edits and the request kinds.

pub const HTTP_TABS: &[&str] = &["Params", "Headers", "Body", "Auth", "Scripts", "Options"];

pub const GRPC_TABS: &[&str] = &["Message", "Metadata", "Auth", "Scripts", "Options"];

pub const WS_TABS: &[&str] = &["Headers", "Messages", "Auth", "Scripts", "Options"];

pub const MCP_TABS: &[&str] = &["Tool", "Args", "Scripts", "Options"];

pub const RESP_TABS: &[&str] = &["Body", "Headers", "Timing", "Results"];

/// Response tab names: the fourth holds assertions, extracted variables and
/// notes ("Results"); for gRPC and MCP it also holds the message log ("Log").
pub fn resp_tab_names(kind: Kind) -> [&'static str; 4] {
    match kind {
        Kind::Http => ["Body", "Headers", "Timing", "Results"],
        Kind::Grpc => ["Body", "Metadata", "Timing", "Log"],
        Kind::Ws => ["Body", "Handshake", "Timing", "Log"],
        Kind::Mcp => ["Body", "Request", "Timing", "Log"],
    }
}

pub const SOURCES: &[&str] = &["jsonpath", "header", "status"];

/// Assertion operators as shown in the table (same order as `AssertOp::ALL`).
pub const OPS: &[&str] = &[
    "==",
    "!=",
    "contains",
    "!contains",
    "exists",
    "!exists",
    "<",
    "<=",
    ">",
    ">=",
    "matches",
];

pub const PART_KINDS: &[&str] = &["text", "file"];

/// A focusable field in the request editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Params,
    Headers,
    BodyKind,
    BodyText,
    RawContentType,
    FormFields,
    Parts,
    FilePath,
    FileContentType,
    AuthKind,
    AuthUser,
    AuthPass,
    AuthToken,
    AuthOAuthName,
    AuthKey,
    AuthValue,
    AuthIn,
    ScriptSection,
    ScriptPre,
    ScriptExtract,
    ScriptAssert,
    Timeout,
    FollowRedirects,
    VerifyTls,
    Cookies,
    Message,
    Metadata,
    SchemaSource,
    ProtoFiles,
    ImportPaths,
    Tls,
    GrpcVerify,
    Deadline,
    McpServer,
    McpTool,
    WsSubprotocols,
    WsMessages,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Params => "Query parameters",
            Field::Headers => "Headers",
            Field::BodyKind => "Body type",
            Field::BodyText => "Body",
            Field::RawContentType => "Content-Type",
            Field::FormFields => "Fields",
            Field::Parts => "Parts",
            Field::FilePath => "File path",
            Field::FileContentType => "Content-Type",
            Field::AuthKind => "Auth type",
            Field::AuthUser => "Username",
            Field::AuthPass => "Password",
            Field::AuthToken => "Token",
            Field::AuthOAuthName => "OAuth identity",
            Field::AuthKey => "Key name",
            Field::AuthValue => "Key value",
            Field::AuthIn => "Send in",
            Field::ScriptSection => "Section",
            Field::ScriptPre => "Pre-request: set variables",
            Field::ScriptExtract => "After response: extract into variables",
            Field::ScriptAssert => "After response: assertions",
            Field::Timeout => "Timeout (ms, empty = default)",
            Field::FollowRedirects => "Follow redirects",
            Field::VerifyTls => "Verify TLS certificates",
            Field::Cookies => "Cookie jar",
            Field::Message => {
                "Request message (JSON; an array sends several on client/bidi streams)"
            }
            Field::Metadata => "Metadata",
            Field::SchemaSource => "Schema from",
            Field::ProtoFiles => ".proto files (comma-separated, relative to the collection)",
            Field::ImportPaths => "Import paths (comma-separated)",
            Field::Tls => "Use TLS",
            Field::GrpcVerify => "Verify TLS certificates",
            Field::Deadline => "Deadline (ms, empty = none)",
            Field::McpServer => "Server (a [mcp.servers] entry of the config)",
            Field::McpTool => "Tool",
            Field::WsSubprotocols => "Subprotocols (comma-separated, offered in order)",
            Field::WsMessages => {
                "Messages sent after connecting (JSON: strings, or {\"binary\": \"base64\"})"
            }
        }
    }

    pub fn is_table(self) -> bool {
        matches!(
            self,
            Field::Params
                | Field::Headers
                | Field::FormFields
                | Field::Parts
                | Field::ScriptPre
                | Field::ScriptExtract
                | Field::ScriptAssert
                | Field::Metadata
        )
    }

    pub fn is_multiline(self) -> bool {
        matches!(self, Field::BodyText | Field::Message | Field::WsMessages)
    }

    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::BodyText
                | Field::RawContentType
                | Field::FilePath
                | Field::FileContentType
                | Field::AuthUser
                | Field::AuthPass
                | Field::AuthToken
                | Field::AuthOAuthName
                | Field::AuthKey
                | Field::AuthValue
                | Field::Timeout
                | Field::Message
                | Field::ProtoFiles
                | Field::ImportPaths
                | Field::Deadline
                | Field::McpServer
                | Field::McpTool
                | Field::WsSubprotocols
                | Field::WsMessages
        )
    }

    /// Choice fields cycle with Space/Enter/Left/Right; toggles flip.
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Field::BodyKind
                | Field::AuthKind
                | Field::AuthIn
                | Field::SchemaSource
                | Field::ScriptSection
                | Field::Cookies
        )
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            Field::FollowRedirects | Field::VerifyTls | Field::Tls | Field::GrpcVerify
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Http,
    Grpc,
    Ws,
    Mcp,
}
