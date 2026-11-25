//! The request fields a tab edits and the request kinds.

pub const HTTP_TABS: &[&str] = &["Params", "Headers", "Body", "Auth", "Scripts", "Options"];

pub const GRPC_TABS: &[&str] = &["Message", "Metadata", "Auth", "Scripts", "Options"];

pub const SSE_TABS: &[&str] = &["Params", "Headers", "Auth", "Scripts", "Options"];

pub const WS_TABS: &[&str] = &["Headers", "Messages", "Auth", "Scripts", "Options"];

pub const MQTT_TABS: &[&str] = &["Topics", "Publish", "Will", "Auth", "Scripts", "Options"];

pub const MCP_TABS: &[&str] = &["Tool", "Args", "Scripts", "Options"];

/// The tabs of a folder's settings (`folder.toml`).
pub const FOLDER_TABS: &[&str] = &["Headers", "Auth", "Vars", "Scripts"];

pub const RESP_TABS: &[&str] = &["Body", "Headers", "Timing", "Results"];

/// Response tab names: the fourth holds assertions, extracted variables and
/// notes ("Results"); for gRPC and MCP it also holds the message log ("Log").
pub fn resp_tab_names(kind: Kind) -> [&'static str; 4] {
    match kind {
        Kind::Http | Kind::Folder => ["Body", "Headers", "Timing", "Results"],
        Kind::Grpc => ["Body", "Metadata", "Timing", "Log"],
        Kind::Sse => ["Body", "Headers", "Timing", "Log"],
        Kind::Ws => ["Body", "Handshake", "Timing", "Log"],
        Kind::Mcp => ["Body", "Request", "Timing", "Log"],
        Kind::Mqtt => ["Messages", "Connection", "Timing", "Log"],
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

/// MQTT QoS levels as shown in the tables and the Last Will choice.
pub const QOS: &[&str] = &["0", "1", "2"];

/// A focusable field in the request editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Params,
    Headers,
    BodyKind,
    BodyText,
    GraphqlSection,
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
    MaxEvents,
    McpServer,
    McpTool,
    WsSubprotocols,
    WsMessages,
    /// A folder's variables.
    Vars,
    AwsAccessKey,
    AwsSecretKey,
    AwsSessionToken,
    AwsRegion,
    AwsService,
    JwtAlgorithm,
    JwtSecret,
    JwtPrivateKey,
    JwtClaims,
    JwtExpires,
    MqttSubscriptions,
    MqttPublish,
    MqttWillTopic,
    MqttWillPayload,
    MqttWillQos,
    MqttWillRetain,
    MqttWillBase64,
    MqttUser,
    MqttPass,
    MqttClientId,
    MqttProtocol,
    MqttCleanSession,
    MqttKeepAlive,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Params => "Query parameters",
            Field::Headers => "Headers",
            Field::BodyKind => "Body type",
            Field::BodyText => "Body",
            Field::GraphqlSection => "GraphQL section",
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
            Field::MaxEvents => "Maximum events (empty = none)",
            Field::Deadline => "Deadline (ms, empty = none)",
            Field::McpServer => "Server (a [mcp.servers] entry of the config)",
            Field::McpTool => "Tool",
            Field::WsSubprotocols => "Subprotocols (comma-separated, offered in order)",
            Field::WsMessages => {
                "Messages sent after connecting (JSON: strings, or {\"binary\": \"base64\"})"
            }
            Field::Vars => "Variables (above the environment, below runtime values)",
            Field::AwsAccessKey => "Access key ID",
            Field::AwsSecretKey => "Secret access key",
            Field::AwsSessionToken => "Session token (temporary credentials only)",
            Field::AwsRegion => "Region",
            Field::AwsService => "Service (e.g. execute-api, s3)",
            Field::JwtAlgorithm => "Algorithm",
            Field::JwtSecret => "Secret (HS256/384/512)",
            Field::JwtPrivateKey => "Private key PEM file (relative to the collection)",
            Field::JwtClaims => "Claims (JSON object)",
            Field::JwtExpires => "Expires in (seconds, adds iat and exp; empty = none)",
            Field::MqttSubscriptions => "Subscriptions (topic filters, + and # wildcards)",
            Field::MqttPublish => {
                "Publishes sent after subscribing (Send publishes the selected row live)"
            }
            Field::MqttWillTopic => "Last Will topic (empty = no Last Will)",
            Field::MqttWillPayload => "Last Will payload",
            Field::MqttWillQos => "Last Will QoS",
            Field::MqttWillRetain => "Last Will retain",
            Field::MqttWillBase64 => "Last Will payload is base64",
            Field::MqttUser => "MQTT user name (empty = from Basic auth)",
            Field::MqttPass => "MQTT password",
            Field::MqttClientId => "Client ID (empty = sendito-<8 hex>)",
            Field::MqttProtocol => "Protocol version",
            Field::MqttCleanSession => "Clean session",
            Field::MqttKeepAlive => "Keep alive (seconds)",
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
                | Field::Vars
                | Field::MqttSubscriptions
                | Field::MqttPublish
        )
    }

    pub fn is_multiline(self) -> bool {
        matches!(
            self,
            Field::BodyText
                | Field::Message
                | Field::WsMessages
                | Field::JwtClaims
                | Field::MqttWillPayload
        )
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
                | Field::MaxEvents
                | Field::Deadline
                | Field::McpServer
                | Field::McpTool
                | Field::WsSubprotocols
                | Field::WsMessages
                | Field::AwsAccessKey
                | Field::AwsSecretKey
                | Field::AwsSessionToken
                | Field::AwsRegion
                | Field::AwsService
                | Field::JwtSecret
                | Field::JwtPrivateKey
                | Field::JwtClaims
                | Field::JwtExpires
                | Field::MqttWillTopic
                | Field::MqttWillPayload
                | Field::MqttUser
                | Field::MqttPass
                | Field::MqttClientId
                | Field::MqttKeepAlive
        )
    }

    /// Choice fields cycle with Space/Enter/Left/Right; toggles flip.
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Field::BodyKind
                | Field::GraphqlSection
                | Field::AuthKind
                | Field::AuthIn
                | Field::SchemaSource
                | Field::ScriptSection
                | Field::Cookies
                | Field::JwtAlgorithm
                | Field::MqttWillQos
                | Field::MqttProtocol
        )
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            Field::FollowRedirects
                | Field::VerifyTls
                | Field::Tls
                | Field::GrpcVerify
                | Field::MqttWillRetain
                | Field::MqttWillBase64
                | Field::MqttCleanSession
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Http,
    Grpc,
    Ws,
    Sse,
    Mqtt,
    Mcp,
    /// The settings of a folder, not a request.
    Folder,
}
