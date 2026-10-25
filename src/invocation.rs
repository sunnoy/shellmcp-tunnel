use serde::{Deserialize, Serialize};

/// Core input created by an admission adapter, never by MCP tool arguments.
#[derive(Clone, Serialize, Deserialize)]
pub struct Invocation {
    pub invocation_id: String,
    pub tool: String,
    pub arguments_json: String,
    pub(crate) authorization: Authorization,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum Authorization {
    Local {
        instance_id: String,
        fingerprint: String,
    },
}

pub fn result(text: impl Into<String>, error: bool) -> serde_json::Value {
    serde_json::json!({"content":[{"type":"text","text":text.into()}],"isError":error})
}
