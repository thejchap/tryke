use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcErrorDetail>,
}

#[derive(Debug, Deserialize)]
pub struct RpcErrorDetail {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub traceback: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunTestParams {
    pub module: String,
    pub function: String,
}

#[derive(Debug, Serialize)]
pub struct ReloadParams {
    pub modules: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunTestResultWire {
    Passed {
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    Failed {
        duration_ms: u64,
        message: String,
        #[serde(default)]
        traceback: Option<String>,
        #[serde(default)]
        assertions: Vec<AssertionWire>,
        stdout: String,
        stderr: String,
    },
    Skipped {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
        stdout: String,
        stderr: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct AssertionWire {
    pub expression: String,
    pub expected: String,
    pub received: String,
    pub line: u32,
    #[serde(default)]
    pub file: Option<String>,
}
