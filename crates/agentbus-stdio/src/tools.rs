//! MCP tool registry: name -> JSON Schema input + dispatch onto the Store.
//! Tool names match v0.1; `register` gains persistent/on_delivery and loses
//! mailbox_size (spool files are unbounded); await/check return envelope
//! BATCHES (fr:08 v0.2 surface).

use std::time::Duration;

use serde_json::{json, Value};

use agentbus_core::store::{RegisterOpts, Store, StoreError};

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Non-persistent ids registered through this shim process; released on EOF.
#[derive(Default)]
pub struct Session {
    registered: Vec<String>,
}

impl Session {
    pub fn cleanup(&mut self, store: &mut Store) {
        for id in self.registered.drain(..) {
            let _ = store.unregister(&id);
        }
    }
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
                    "persistent": {"type": "boolean"},
                    "on_delivery": {"type": "string"}
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
            description: "List registered instances.",
            input_schema: json!({"type": "object"}),
        },
        ToolSpec {
            name: "await_message",
            description: "Block until messages arrive for instance_id, or time out (empty list).",
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
            description:
                "Send a request to another instance and block until it replies or times out.",
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
            description: "Reply to an inbound ask; the asker reads it via the asks row.",
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
            description: "Append a broadcast event to the ordered event log.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "payload": {}
                },
                "required": ["from", "payload"]
            }),
        },
    ]
}

pub fn call(
    store: &mut Store,
    session: &mut Session,
    name: &str,
    mut args: Value,
) -> Result<Value, Value> {
    normalize_json_string_field(&mut args, "payload");
    match name {
        "register" => {
            let id = str_arg(&args, "instance_id")?;
            let persistent = args
                .get("persistent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let opts = RegisterOpts {
                persistent,
                on_delivery: args
                    .get("on_delivery")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                pid: None,
            };
            store.register(&id, &opts).map_err(store_err)?;
            if !persistent {
                session.registered.push(id);
            }
            Ok(json!({"ok": true}))
        }
        "unregister" => {
            let id = str_arg(&args, "instance_id")?;
            let removed = store.unregister(&id).map_err(store_err)?;
            session.registered.retain(|r| *r != id);
            Ok(json!({"ok": removed}))
        }
        "list_instances" => {
            let rows = store.list_instances().map_err(store_err)?;
            Ok(json!({ "instances": rows }))
        }
        "await_message" => {
            let id = str_arg(&args, "instance_id")?;
            let timeout = timeout_arg(&args, 30_000);
            let envelopes = store.await_message(&id, timeout).map_err(store_err)?;
            Ok(json!({ "envelopes": envelopes }))
        }
        "check_inbox" => {
            let id = str_arg(&args, "instance_id")?;
            let envelopes = store.check_inbox(&id).map_err(store_err)?;
            Ok(json!({ "envelopes": envelopes }))
        }
        "send" => {
            let from = str_arg(&args, "from")?;
            let to = str_arg(&args, "to")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let delivered = store.send(&from, &to, payload).map_err(store_err)?;
            Ok(json!({"id": delivered.envelope.id, "hook": delivered.hook}))
        }
        "ask" => {
            let from = str_arg(&args, "from")?;
            let to = str_arg(&args, "to")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let timeout = timeout_arg(&args, 30_000);
            let reply = store.ask(&from, &to, payload, timeout).map_err(store_err)?;
            Ok(json!({"request_id": reply.request_id, "payload": reply.payload}))
        }
        "reply" => {
            let from = str_arg(&args, "from")?;
            let request_id = str_arg(&args, "request_id")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            store
                .reply(&from, &request_id, payload)
                .map_err(store_err)?;
            Ok(json!({"ok": true}))
        }
        "publish_event" => {
            let from = str_arg(&args, "from")?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let id = store.publish_event(&from, payload).map_err(store_err)?;
            Ok(json!({ "id": id }))
        }
        other => Err(json!({"code": -32601, "message": format!("unknown tool `{other}`")})),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<String, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| json!({"code": -32602, "message": format!("missing `{key}`")}))
}

fn timeout_arg(args: &Value, default_ms: u64) -> Duration {
    Duration::from_millis(
        args.get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(default_ms),
    )
}

fn store_err(e: StoreError) -> Value {
    json!({"code": -32000, "message": e.code(), "data": e.to_string()})
}

fn normalize_json_string_field(args: &mut Value, key: &str) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    let Some(field) = obj.get_mut(key) else {
        return;
    };
    if let Some(s) = field.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(s) {
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
