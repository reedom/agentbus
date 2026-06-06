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
            description: "Reply to an inbound ask. `to` is auto-filled from the original ask's sender if omitted.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "request_id": {"type": "string"},
                    "to": {"type": "string"},
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
    mut args: serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    // Some MCP clients serialize permissive ({} schema) fields as JSON
    // strings instead of native values. Auto-parse so the daemon stores
    // payload as the structured value it represents.
    normalize_json_string_field(&mut args, "payload");
    client.call(name, args).await
}

fn normalize_json_string_field(args: &mut serde_json::Value, key: &str) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    let Some(field) = obj.get_mut(key) else {
        return;
    };
    if let Some(s) = field.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            *field = parsed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_replaces_json_string_with_parsed_value() {
        let mut args = json!({"payload": "{\"text\":\"hi\"}"});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args["payload"], json!({"text": "hi"}));
    }

    #[test]
    fn normalize_keeps_non_json_string_as_is() {
        let mut args = json!({"payload": "plain text"});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args["payload"], json!("plain text"));
    }

    #[test]
    fn normalize_keeps_native_object_unchanged() {
        let mut args = json!({"payload": {"text": "hi"}});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args["payload"], json!({"text": "hi"}));
    }

    #[test]
    fn normalize_no_field_is_noop() {
        let mut args = json!({"other": 1});
        normalize_json_string_field(&mut args, "payload");
        assert_eq!(args, json!({"other": 1}));
    }
}
