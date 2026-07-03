use std::{fmt, path::PathBuf};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tryke_types::{RunSummary, TestItem, TestResult};

#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: RequestMethod,
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(from = "String")]
pub enum RequestMethod {
    Ping,
    Discover,
    DidChange,
    Run,
    Unknown(String),
}

impl From<String> for RequestMethod {
    fn from(method: String) -> Self {
        match method.as_str() {
            "ping" => Self::Ping,
            "discover" => Self::Discover,
            "did_change" => Self::DidChange,
            "run" => Self::Run,
            _ => Self::Unknown(method),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMethod {
    DiscoverComplete,
    RunStart,
    TestComplete,
    RunComplete,
}

impl fmt::Display for NotificationMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiscoverComplete => f.write_str("discover_complete"),
            Self::RunStart => f.write_str("run_start"),
            Self::TestComplete => f.write_str("test_complete"),
            Self::RunComplete => f.write_str("run_complete"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DidChangeParams {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct RunParams {
    pub tests: Option<Vec<String>>,
    pub filter: Option<String>,
    pub paths: Option<Vec<String>>,
    pub markers: Option<String>,
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct Response<T: Serialize> {
    pub jsonrpc: String,
    pub id: Value,
    pub result: T,
}

impl<T: Serialize> Response<T> {
    #[must_use]
    pub fn new(id: Value, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        }
    }

    /// Serializes the response as one newline-delimited JSON-RPC message.
    ///
    /// # Errors
    /// Returns an error if the response payload cannot be serialized.
    pub fn into_json_line(self) -> serde_json::Result<Bytes> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        Ok(Bytes::from(bytes))
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub error: RpcError,
}

impl ErrorResponse {
    #[must_use]
    pub fn new(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            error: RpcError { code, message },
        }
    }

    /// Serializes the error as one newline-delimited JSON-RPC message.
    ///
    /// # Errors
    /// Returns an error if the error response cannot be serialized.
    pub fn into_json_line(self) -> serde_json::Result<Bytes> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        Ok(Bytes::from(bytes))
    }
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Notification<T: Serialize> {
    pub jsonrpc: String,
    pub method: NotificationMethod,
    pub params: T,
}

#[derive(Debug, Serialize)]
pub struct DiscoverCompleteParams {
    pub tests: Vec<TestItem>,
}

#[derive(Debug, Serialize)]
pub struct RunStartParams {
    pub run_id: String,
    pub tests: Vec<TestItem>,
}

#[derive(Debug, Serialize)]
pub struct TestCompleteParams {
    pub run_id: String,
    pub result: TestResult,
}

#[derive(Debug, Serialize)]
pub struct RunCompleteParams {
    pub run_id: String,
    pub summary: RunSummary,
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    pub run_id: String,
    pub summary: RunSummary,
}

pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_ping_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, RequestMethod::Ping);
        assert!(req.id.is_some());
        assert!(req.params.is_none());
    }

    #[test]
    fn deserializes_unknown_request_method() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"custom"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, RequestMethod::Unknown("custom".to_string()));
    }

    #[test]
    fn deserializes_discover_request() {
        // `discover` carries no parameters; the request must still
        // deserialize whether or not a client supplies a `params` field.
        let json = r#"{"jsonrpc":"2.0","id":2,"method":"discover"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, RequestMethod::Discover);
        assert!(req.id.is_some());
        assert!(req.params.is_none());
    }

    #[test]
    fn deserializes_run_request_tests_null() {
        let json = r#"{"jsonrpc":"2.0","id":3,"method":"run","params":{"root":"/tmp","tests":null,"run_id":"r1"}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        let params: RunParams = serde_json::from_value(req.params.unwrap()).unwrap();
        assert!(params.tests.is_none());
        assert_eq!(params.run_id, "r1");
    }

    #[test]
    fn deserializes_run_request_with_tests() {
        let json = r#"{"jsonrpc":"2.0","id":3,"method":"run","params":{"root":"/tmp","tests":["a","b"],"run_id":"r1"}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        let params: RunParams = serde_json::from_value(req.params.unwrap()).unwrap();
        assert_eq!(params.tests, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn serializes_response() {
        let resp = Response {
            jsonrpc: "2.0".to_string(),
            id: serde_json::Value::Number(1.into()),
            result: "pong",
        };
        let val: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(val["result"], "pong");
        assert_eq!(val["jsonrpc"], "2.0");
    }

    #[test]
    fn serializes_error_response() {
        let resp = ErrorResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            error: RpcError {
                code: METHOD_NOT_FOUND,
                message: "not found".to_string(),
            },
        };
        let val: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(val["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(val["error"]["message"], "not found");
    }

    #[test]
    fn serializes_notification() {
        let notif = Notification {
            jsonrpc: "2.0".to_string(),
            method: NotificationMethod::DiscoverComplete,
            params: DiscoverCompleteParams { tests: vec![] },
        };
        let val: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&notif).unwrap()).unwrap();
        assert_eq!(val["method"], "discover_complete");
        assert!(val["params"]["tests"].is_array());
        assert!(val.get("id").is_none());
    }

    #[test]
    fn run_params_without_run_id_is_rejected() {
        let json = r#"{"tests":null}"#;
        let result: Result<RunParams, _> = serde_json::from_str(json);
        assert!(result.is_err(), "run_id is required");
    }

    #[test]
    fn run_params_with_run_id_deserializes() {
        let json = r#"{"tests":null,"run_id":"abc-123"}"#;
        let params: RunParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.run_id, "abc-123");
    }

    #[test]
    fn run_start_params_serialize_with_run_id() {
        let params = RunStartParams {
            run_id: "r1".to_string(),
            tests: vec![],
        };
        let val: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&params).unwrap()).unwrap();
        assert_eq!(val["run_id"], "r1");
        assert!(val["tests"].is_array());
    }
}
