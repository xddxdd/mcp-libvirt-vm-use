//! Minimal MCP server: JSON-RPC 2.0 over stdio.
//!
//! Framing is newline-delimited JSON — exactly one JSON-RPC message per line,
//! which is what MCP stdio uses (the older `Content-Length` framing is not
//! used). stdout carries JSON-RPC responses only; anything diagnostic must go
//! to stderr.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::libvirt::Libvirt;
use crate::tools;

/// Protocol version reported when the client does not ask for a specific one.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "mcp-libvirt";
pub const SERVER_VERSION: &str = "0.1.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;

pub struct McpServer {
    libvirt: Libvirt,
}

impl McpServer {
    pub fn new(libvirt: Libvirt) -> Self {
        McpServer { libvirt }
    }

    /// Read one JSON-RPC message per stdin line until EOF, writing responses
    /// (and nothing else) to stdout.
    pub fn run(&self) -> Result<(), String> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| format!("failed to read stdin: {}", e))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(line) {
                let mut encoded = serde_json::to_vec(&response)
                    .map_err(|e| format!("failed to encode JSON-RPC response: {}", e))?;
                encoded.push(b'\n');
                out.write_all(&encoded)
                    .map_err(|e| format!("failed to write JSON-RPC response to stdout: {}", e))?;
                out.flush()
                    .map_err(|e| format!("failed to flush stdout: {}", e))?;
            }
        }
        Ok(())
    }

    /// Handle one line of input. Returns `Some(response)` when a reply is due,
    /// `None` for notifications and blank input.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => {
                return Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {}", e),
                ))
            }
        };
        self.handle_message(message)
    }

    pub fn handle_message(&self, message: Value) -> Option<Value> {
        let object = match message {
            Value::Object(object) => object,
            _ => {
                return Some(error_response(
                    Value::Null,
                    INVALID_REQUEST,
                    "invalid request: expected a JSON object",
                ))
            }
        };

        let id = object.get("id").filter(|value| !value.is_null()).cloned();
        let method = match object.get("method").and_then(|value| value.as_str()) {
            Some(method) => method.to_string(),
            None => {
                return id.map(|id| {
                    error_response(id, INVALID_REQUEST, "invalid request: missing 'method' string")
                })
            }
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);

        match self.dispatch(&method, &params) {
            Some(Ok(result)) => id.map(|id| success_response(id, result)),
            Some(Err((code, message))) => id.map(|id| error_response(id, code, &message)),
            // Notifications are never answered.
            None => None,
        }
    }

    fn dispatch(&self, method: &str, params: &Value) -> Option<Result<Value, (i64, String)>> {
        match method {
            "initialize" => Some(Ok(self.initialize_result(params))),
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({ "tools": tools::tool_definitions() }))),
            "tools/call" => Some(Ok(self.tools_call_result(params))),
            // e.g. notifications/initialized, notifications/cancelled.
            _ if method.starts_with("notifications/") => None,
            _ => Some(Err((
                METHOD_NOT_FOUND,
                format!("method not found: {}", method),
            ))),
        }
    }

    fn initialize_result(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(|value| value.as_str())
            .filter(|version| !version.is_empty())
            .unwrap_or(DEFAULT_PROTOCOL_VERSION);
        json!({
            "protocolVersion": requested,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        })
    }

    fn tools_call_result(&self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let arguments = params.get("arguments");
        let output = tools::call_tool(&self.libvirt, name, arguments);
        let mut result = json!({ "content": output.content });
        if output.is_error {
            result["isError"] = json!(true);
        }
        result
    }
}

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer::new(Libvirt {
            uri: "qemu:///system".to_string(),
        })
    }

    #[test]
    fn initialize_echoes_client_protocol_version() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}}"#;
        let response = server().handle_line(line).expect("initialize needs a reply");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(response["result"]["serverInfo"]["name"], "mcp-libvirt");
        assert_eq!(response["result"]["serverInfo"]["version"], "0.1.0");
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_without_version_uses_default() {
        let line = r#"{"jsonrpc":"2.0","id":"abc","method":"initialize","params":{}}"#;
        let response = server().handle_line(line).expect("initialize needs a reply");
        assert_eq!(response["id"], "abc");
        assert_eq!(
            response["result"]["protocolVersion"],
            DEFAULT_PROTOCOL_VERSION
        );
    }

    #[test]
    fn initialized_notification_has_no_reply() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(server().handle_line(line).is_none());
        let cancelled = r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        assert!(server().handle_line(cancelled).is_none());
    }

    #[test]
    fn ping_returns_empty_object() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#;
        let response = server().handle_line(line).expect("ping needs a reply");
        assert_eq!(response["result"], json!({}));
    }

    #[test]
    fn tools_list_returns_all_tools() {
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let response = server().handle_line(line).expect("tools/list needs a reply");
        let tools = response["result"]["tools"]
            .as_array()
            .expect("tools must be an array");
        assert_eq!(tools.len(), 8);
        assert_eq!(tools[0]["name"], "list_domains");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_call_unknown_tool_is_an_error_result() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#;
        let response = server().handle_line(line).expect("tools/call needs a reply");
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["content"][0]["type"], "text");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("error text");
        assert!(text.contains("unknown tool"), "got: {}", text);
    }

    #[test]
    fn tools_call_without_name_is_an_error_result() {
        let line = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{}}"#;
        let response = server().handle_line(line).expect("tools/call needs a reply");
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn unknown_method_reports_method_not_found() {
        let line = r#"{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}"#;
        let response = server().handle_line(line).expect("request needs a reply");
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(response["id"], 5);
        let text = response["error"]["message"].as_str().expect("message");
        assert!(text.contains("resources/list"), "got: {}", text);
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let response = server().handle_line("{not json").expect("parse error needs a reply");
        assert_eq!(response["error"]["code"], PARSE_ERROR);
        assert!(response["id"].is_null());
    }

    #[test]
    fn non_object_message_is_an_invalid_request() {
        let response = server().handle_line("[1,2,3]").expect("invalid request needs a reply");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn request_without_method_is_an_invalid_request() {
        let response = server()
            .handle_line(r#"{"jsonrpc":"2.0","id":6}"#)
            .expect("invalid request needs a reply");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
        assert_eq!(response["id"], 6);
    }

    #[test]
    fn response_line_is_single_line_json() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
        let response = server().handle_line(line).expect("ping needs a reply");
        let encoded = serde_json::to_string(&response).expect("encodable");
        assert!(!encoded.contains('\n'));
    }
}
