use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tryke_types::{RunSummary, TestItem, TestResult};

#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverParams {
    pub root: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
pub struct RunParams {
    pub tests: Option<Vec<String>>,
    pub filter: Option<String>,
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct Response<T: Serialize> {
    pub jsonrpc: String,
    pub id: Value,
    pub result: T,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub error: RpcError,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Notification<T: Serialize> {
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

#[derive(Debug, Serialize)]
pub struct DiscoverCompleteParams {
    pub tests: Vec<TestItem>,
}

#[derive(Debug, Serialize)]
pub struct RunStartParams {
    pub tests: Vec<TestItem>,
}

#[derive(Debug, Serialize)]
pub struct TestCompleteParams {
    pub result: TestResult,
}

#[derive(Debug, Serialize)]
pub struct RunCompleteParams {
    pub summary: RunSummary,
}

pub const METHOD_NOT_FOUND: i32 = -32601;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_ping_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "ping");
        assert!(req.id.is_some());
        assert!(req.params.is_none());
    }

    #[test]
    fn deserializes_discover_request() {
        let json = r#"{"jsonrpc":"2.0","id":2,"method":"discover","params":{"root":"/tmp"}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        let params: DiscoverParams = serde_json::from_value(req.params.unwrap()).unwrap();
        assert_eq!(params.root, PathBuf::from("/tmp"));
    }

    #[test]
    fn deserializes_run_request_tests_null() {
        let json =
            r#"{"jsonrpc":"2.0","id":3,"method":"run","params":{"root":"/tmp","tests":null}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        let params: RunParams = serde_json::from_value(req.params.unwrap()).unwrap();
        assert!(params.tests.is_none());
    }

    #[test]
    fn deserializes_run_request_with_tests() {
        let json =
            r#"{"jsonrpc":"2.0","id":3,"method":"run","params":{"root":"/tmp","tests":["a","b"]}}"#;
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
            method: "discover_complete".to_string(),
            params: DiscoverCompleteParams { tests: vec![] },
        };
        let val: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&notif).unwrap()).unwrap();
        assert_eq!(val["method"], "discover_complete");
        assert!(val["params"]["tests"].is_array());
        assert!(val.get("id").is_none());
    }
}
