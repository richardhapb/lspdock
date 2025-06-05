use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum LspMessage {
    Request {
        id: i32,
        method: String,
        params: serde_json::Value,
    },
    Response {
        id: i32,
        result: Option<serde_json::Value>,
        error: Option<serde_json::Value>
    },
    Notification {
        method: String,
        params: serde_json::Value,
    },
}
