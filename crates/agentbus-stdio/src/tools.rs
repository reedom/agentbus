//! MCP tool registry: name -> JSON Schema input + handler that calls the UDS client.

use std::sync::Arc;

use serde_json::json;

use crate::uds_client::{ClientError, UdsClient};

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "register",
            description: "Register this session under an instance_id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instance_id": {"type": "string"},
                    "mailbox_size": {"type": "integer", "minimum": 1}
                },
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "unregister",
            description: "Release a previously registered instance_id.",
            input_schema: json!({
                "type": "object",
                "properties": {"instance_id": {"type": "string"}},
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "list_instances",
            description: "List active instances.",
            input_schema: json!({"type": "object"}),
        },
        ToolSpec {
            name: "await_message",
            description: "Block until a message arrives for instance_id, or time out.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instance_id": {"type": "string"},
                    "timeout_ms": {"type": "integer", "minimum": 1}
                },
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "check_inbox",
            description: "Drain pending messages for instance_id without blocking.",
            input_schema: json!({
                "type": "object",
                "properties": {"instance_id": {"type": "string"}},
                "required": ["instance_id"]
            }),
        },
        ToolSpec {
            name: "send",
            description: "Send a one-way message to another instance.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "to", "payload"]
            }),
        },
        ToolSpec {
            name: "ask",
            description: "Send a request to another instance and block until it replies or times out.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "payload": {},
                    "timeout_ms": {"type": "integer"}
                },
                "required": ["from", "to", "payload"]
            }),
        },
        ToolSpec {
            name: "reply",
            description: "Reply to an inbound ask.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "request_id": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "request_id", "payload"]
            }),
        },
        ToolSpec {
            name: "publish_event",
            description: "Broadcast an event to all SSE subscribers.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "kind": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "payload"]
            }),
        },
    ]
}

pub async fn call(
    client: &Arc<UdsClient>,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    client.call(name, args).await
}
