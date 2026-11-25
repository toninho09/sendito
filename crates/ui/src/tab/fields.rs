//! The request fields a tab edits and the request kinds.

pub const HTTP_TABS: &[&str] = &[
    "Params", "Headers", "Body", "Auth", "Scripts", "Options", "Docs",
];

pub const GRPC_TABS: &[&str] = &["Message", "Metadata", "Auth", "Scripts", "Options", "Docs"];

pub const SSE_TABS: &[&str] = &["Params", "Headers", "Auth", "Scripts", "Options", "Docs"];

pub const WS_TABS: &[&str] = &["Headers", "Messages", "Auth", "Scripts", "Options", "Docs"];

pub const MQTT_TABS: &[&str] = &[
    "Topics", "Publish", "Will", "Auth", "Scripts", "Options", "Docs",
];

pub const MCP_TABS: &[&str] = &["Tool", "Args", "Scripts", "Options", "Docs"];

/// The tabs of a folder's settings (`folder.toml`).
pub const FOLDER_TABS: &[&str] = &["Headers", "Auth", "Vars", "Scripts", "Docs"];

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

/// Script sources as shown in the table (same order as `Source::ALL`).
pub const SOURCES: &[&str] = &[
    "jsonpath", "header", "status", "time", "size", "hash", "xpath", "regex", "cookie",
];

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
    /// The Lua script of the selected section (pre or post).
    ScriptCode,
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
    /// The Docs tab view: edit the Markdown or preview it.
    DocsView,
    /// The Markdown description, as text.
    Description,
    /// The Markdown description, rendered (read only).
    DocsPreview,
}

impl Field {
    /// The short name drawn before a one-line field (and the title of its
    /// option list). Tables and multi-line editors fill their area and have
    /// none, except a folder's variables.
    pub fn label(self) -> &'static str {
        match self {
            Field::BodyKind => "Body type",
            Field::GraphqlSection | Field::ScriptSection => "Section",
            Field::RawContentType | Field::FileContentType => "Content-Type",
            Field::FilePath => "File",
            Field::AuthKind => "Auth type",
            Field::AuthUser => "Username",
            Field::AuthPass | Field::MqttPass => "Password",
            Field::AuthToken => "Token",
            Field::AuthOAuthName => "OAuth identity",
            Field::AuthKey => "Key name",
            Field::AuthValue => "Key value",
            Field::AuthIn => "Send in",
            Field::Timeout => "Timeout",
            Field::FollowRedirects => "Redirects",
            Field::VerifyTls | Field::GrpcVerify => "Verify TLS",
            Field::Cookies => "Cookie jar",
            Field::SchemaSource => "Schema",
            Field::ProtoFiles => "Proto files",
            Field::ImportPaths => "Import paths",
            Field::Tls => "Use TLS",
            Field::Deadline => "Deadline",
            Field::MaxEvents => "Max events",
            Field::McpServer => "Server",
            Field::McpTool => "Tool",
            Field::WsSubprotocols => "Subprotocols",
            Field::Vars => "Variables",
            Field::AwsAccessKey => "Access key",
            Field::AwsSecretKey => "Secret key",
            Field::AwsSessionToken => "Session token",
            Field::AwsRegion => "Region",
            Field::AwsService => "Service",
            Field::JwtAlgorithm => "Algorithm",
            Field::JwtSecret => "Secret",
            Field::JwtPrivateKey => "Private key",
            Field::JwtExpires => "Expires in s",
            Field::MqttWillTopic => "Will topic",
            Field::MqttWillQos => "Will QoS",
            Field::MqttWillRetain => "Will retain",
            Field::MqttWillBase64 => "Base64",
            Field::MqttUser => "User name",
            Field::MqttClientId => "Client ID",
            Field::MqttProtocol => "Protocol",
            Field::MqttCleanSession => "Clean session",
            Field::MqttKeepAlive => "Keep alive s",
            Field::DocsView => "View",
            Field::Params
            | Field::Headers
            | Field::BodyText
            | Field::FormFields
            | Field::Parts
            | Field::ScriptPre
            | Field::ScriptExtract
            | Field::ScriptAssert
            | Field::ScriptCode
            | Field::Message
            | Field::Metadata
            | Field::WsMessages
            | Field::JwtClaims
            | Field::MqttSubscriptions
            | Field::MqttPublish
            | Field::MqttWillPayload
            | Field::Description
            | Field::DocsPreview => "",
        }
    }

    /// What a one-line field does, for the help line under the fields while
    /// it has the focus. Empty for tables and multi-line editors, which have
    /// notes of their own. `kind` is the request kind: a timeout means a
    /// different wait for each protocol.
    pub fn help(self, kind: Kind) -> &'static str {
        const SECRET: &str = "Keep it in an environment secret and write {{name}} here.";
        match self {
            Field::BodyKind => {
                "The format of the body. The Content-Type on the right is the one Sendito sends."
            }
            Field::GraphqlSection => {
                "Edit the query, its variables (a JSON object) or the operation name."
            }
            Field::RawContentType => {
                "The Content-Type header of the body. Empty sends no Content-Type."
            }
            Field::FilePath => "The file sent as the body, relative to the collection folder.",
            Field::FileContentType => {
                "The Content-Type header of the file. Empty sends application/octet-stream."
            }
            Field::AuthKind => {
                "How the request proves who it is. Inherit uses the auth of the nearest folder."
            }
            Field::AuthUser | Field::AwsAccessKey => "Values may use {{variables}}.",
            Field::AuthPass
            | Field::AuthValue
            | Field::AwsSecretKey
            | Field::JwtSecret
            | Field::MqttPass => SECRET,
            Field::AuthToken => {
                "Sent as Authorization: Bearer <token>. Keep it in an environment secret and write {{name}} here."
            }
            Field::AuthOAuthName => {
                "An [oauth.<name>] identity of the config. F2 lists them and adds a new one."
            }
            Field::AuthKey => "The name of the header or query parameter that carries the key.",
            Field::AuthIn => "Send the key in a header or as a query parameter.",
            Field::ScriptSection => {
                "Variables set before the request, values extracted from the response, or assertions."
            }
            Field::Timeout => match kind {
                Kind::Sse => {
                    "The longest wait for the stream to open: 30 s, 500 ms (a bare number is ms)."
                }
                Kind::Ws => {
                    "The longest wait for the handshake: 10 s, 500 ms (a bare number is ms)."
                }
                Kind::Mqtt => {
                    "The longest wait for the broker's CONNACK: 10 s (a bare number is ms)."
                }
                Kind::Mcp => {
                    "The longest time for the whole MCP session: 30 s, 2 min (a bare number is ms)."
                }
                _ => "The longest time the request may take: 5 s, 500 ms (a bare number is ms).",
            },
            Field::Deadline => match kind {
                Kind::Grpc => {
                    "The longest time the call may take, sent as grpc-timeout (a bare number is ms)."
                }
                Kind::Sse => {
                    "Ends the stream this long after it opens: 1 min, 30 s. Empty = no deadline."
                }
                _ => "Ends the whole session after this time: 1 min, 30 s. Empty = no deadline.",
            },
            Field::FollowRedirects => "Off returns the 3xx response as it is.",
            Field::VerifyTls | Field::GrpcVerify => {
                "Off accepts any certificate. Use it only for self-signed test servers."
            }
            Field::Cookies => {
                "Send and keep the cookies of the active environment. Config default uses the Settings value."
            }
            Field::SchemaSource => {
                "Server reflection asks the server for the schema; proto files are compiled locally."
            }
            Field::ProtoFiles => {
                "The .proto files, separated by commas, relative to the collection folder."
            }
            Field::ImportPaths => {
                "Folders for proto imports, separated by commas. Empty uses the folder of each file."
            }
            Field::Tls => "Use TLS when the target has no scheme.",
            Field::MaxEvents => "Ends the stream after this many events. Empty = no limit.",
            Field::McpServer => "A server name from [mcp.servers] or an http(s):// URL.",
            Field::McpTool => "The tool to call on the server.",
            Field::WsSubprotocols => {
                "Offered in Sec-WebSocket-Protocol in this order, separated by commas."
            }
            Field::AwsSessionToken => "Only for temporary credentials. Empty for long-term keys.",
            Field::AwsRegion => "The AWS region of the service, e.g. us-east-1.",
            Field::AwsService => "The AWS service that signs the request, e.g. execute-api or s3.",
            Field::JwtAlgorithm => {
                "HS256, HS384 and HS512 sign with a secret; RS256 and ES256 sign with a PEM key file."
            }
            Field::JwtPrivateKey => {
                "The PEM file of the private key, relative to the collection folder."
            }
            Field::JwtExpires => {
                "Seconds until the token expires; adds the iat and exp claims. Empty = no expiry."
            }
            Field::MqttWillTopic => {
                "The broker publishes the Last Will here when the client drops. Empty = no Last Will."
            }
            Field::MqttWillQos => "The delivery guarantee of the Last Will: 0, 1 or 2.",
            Field::MqttWillRetain => "The broker keeps the Last Will for new subscribers.",
            Field::MqttWillBase64 => {
                "The payload is base64 text; the broker gets the decoded bytes."
            }
            Field::MqttUser => "The MQTT user name. Empty uses the Basic auth below.",
            Field::MqttClientId => {
                "The client ID the broker sees. Empty sends sendito- and 8 random hex digits."
            }
            Field::MqttProtocol => "The MQTT version the broker speaks.",
            Field::MqttCleanSession => {
                "Start without the session state the broker kept for this client ID."
            }
            Field::MqttKeepAlive => "Seconds between keep-alive pings. Empty = 60.",
            Field::DocsView => "Edit the Markdown text or read the rendered page.",
            Field::Params
            | Field::Headers
            | Field::BodyText
            | Field::FormFields
            | Field::Parts
            | Field::ScriptPre
            | Field::ScriptExtract
            | Field::ScriptAssert
            | Field::ScriptCode
            | Field::Message
            | Field::Metadata
            | Field::WsMessages
            | Field::Vars
            | Field::JwtClaims
            | Field::MqttSubscriptions
            | Field::MqttPublish
            | Field::MqttWillPayload
            | Field::Description
            | Field::DocsPreview => "",
        }
    }

    /// A field whose value is a credential: a plain-text value is saved in
    /// the request file as typed.
    pub fn is_secret(self) -> bool {
        matches!(
            self,
            Field::AuthPass
                | Field::AuthToken
                | Field::AuthValue
                | Field::AwsSecretKey
                | Field::AwsSessionToken
                | Field::JwtSecret
                | Field::MqttPass
        )
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

    /// Tables, multi-line editors and the preview share the free height.
    pub fn is_flex(self) -> bool {
        self.is_table() || self.is_multiline() || self == Field::DocsPreview
    }

    pub fn is_multiline(self) -> bool {
        matches!(
            self,
            Field::BodyText
                | Field::ScriptCode
                | Field::Message
                | Field::WsMessages
                | Field::JwtClaims
                | Field::MqttWillPayload
                | Field::Description
        )
    }

    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::BodyText
                | Field::ScriptCode
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
                | Field::Description
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
                | Field::DocsView
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

/// A credential written as plain text instead of a `{{name}}` reference:
/// the file keeps it as typed.
pub fn plain_secret(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains("{{")
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

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::{AssertOp, Source};

    #[test]
    fn choice_lists_follow_the_model_order() {
        let sources: Vec<_> = Source::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(SOURCES, sources);
        let ops: Vec<_> = AssertOp::ALL.iter().map(|o| o.symbol()).collect();
        assert_eq!(OPS, ops);
    }
}
