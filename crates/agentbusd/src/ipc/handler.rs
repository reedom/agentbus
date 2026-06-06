//! IPC method dispatcher. One `ConnCtx` per connection holds the per-conn
//! owner token so dropping the connection auto-unregisters its instances.

use serde_json::json;
use std::time::Duration;

use agentbus_core::envelope::{Envelope, Kind};
use agentbus_core::ids::{new_envelope_id, now_utc};
use agentbus_core::registry::OwnerToken;
use agentbus_core::router::RouteError;

use super::proto::{RpcError, RpcResponse};
use crate::state::AppState;

pub struct ConnCtx {
    pub owner: OwnerToken,
    pub claimed_ids: Vec<String>,
}

pub async fn dispatch(
    state: &AppState,
    ctx: &mut ConnCtx,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, RpcError> {
    match method {
        "register" => {
            let id = params
                .get("instance_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(-32602, "missing instance_id"))?;
            let mailbox_size = params
                .get("mailbox_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(256) as usize;
            match state.registry.register(id, ctx.owner, mailbox_size).await {
                Ok(_) => {
                    if !ctx.claimed_ids.iter().any(|x| x == id) {
                        ctx.claimed_ids.push(id.into());
                    }
                    Ok(json!({"ok": true}))
                }
                Err(agentbus_core::registry::RegisterError::Collision(_)) => {
                    Err(err(1001, "instance_id_taken"))
                }
                Err(agentbus_core::registry::RegisterError::Invalid) => {
                    Err(err(1002, "invalid_instance_id"))
                }
            }
        }
        "unregister" => {
            let id = params
                .get("instance_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(-32602, "missing instance_id"))?;
            let ok = state.registry.unregister(id, ctx.owner).await;
            if ok {
                state.router.cancel_pending_for(id).await;
                ctx.claimed_ids.retain(|x| x != id);
            }
            Ok(json!({"ok": ok}))
        }
        "list_instances" => {
            let ids = state.registry.list_ids().await;
            Ok(json!({"instances": ids}))
        }
        "await_message" => {
            let id = params
                .get("instance_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(-32602, "missing instance_id"))?;
            let timeout_ms = params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(30_000);
            let rec = state
                .registry
                .lookup(id)
                .await
                .ok_or_else(|| err(1003, "unknown_instance"))?;
            match rec
                .mailbox
                .pop_with_timeout(Duration::from_millis(timeout_ms))
                .await
            {
                Ok(env) => Ok(json!({ "envelope": env })),
                Err(agentbus_core::mailbox::RecvError::Timeout) => {
                    Ok(json!({"envelope": serde_json::Value::Null, "timeout": true}))
                }
                Err(agentbus_core::mailbox::RecvError::Closed) => Err(err(1004, "mailbox_closed")),
            }
        }
        "check_inbox" => {
            let id = params
                .get("instance_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(-32602, "missing instance_id"))?;
            let rec = state
                .registry
                .lookup(id)
                .await
                .ok_or_else(|| err(1003, "unknown_instance"))?;
            let drained = rec.mailbox.drain().await;
            Ok(json!({ "envelopes": drained }))
        }
        "send" => {
            let env = build_envelope(params, Kind::Message)?;
            let id = state
                .router
                .send(env.clone())
                .await
                .map_err(map_route_err)?;
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env);
            Ok(json!({ "id": id }))
        }
        "ask" => {
            let requested = params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(state.cfg.default_timeout_ms);
            // Clamp into [1_000, max_timeout_ms]; using min/max keeps the
            // pre-existing pattern but stays free of `>` / `>=` comparisons.
            let timeout_ms = requested.min(state.cfg.max_timeout_ms).max(1_000);
            let env = build_envelope(params, Kind::Ask)?;
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env.clone());
            let reply = state
                .router
                .ask(env, Duration::from_millis(timeout_ms))
                .await
                .map_err(map_route_err)?;
            let _ = state.log.append(&reply).await;
            let _ = state.broadcast_tx.send(reply.clone());
            Ok(json!({ "reply": reply }))
        }
        "reply" => {
            let mut env = build_envelope(params, Kind::Reply)?;
            env.request_id = params
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(Into::into);
            state
                .router
                .reply(env.clone())
                .await
                .map_err(map_route_err)?;
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env);
            Ok(json!({"ok": true}))
        }
        "publish_event" => {
            let env = Envelope {
                id: new_envelope_id(),
                kind: Kind::Event,
                from: params
                    .get("from")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .into(),
                to: None,
                request_id: None,
                timeout_ms: None,
                ts: now_utc(),
                payload: serde_json::json!({
                    "type": params.get("kind").and_then(|v| v.as_str()).unwrap_or("event"),
                    "data": params.get("payload").cloned().unwrap_or(serde_json::Value::Null),
                }),
            };
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env.clone());
            Ok(json!({ "id": env.id }))
        }
        _ => Err(err(-32601, "method not found")),
    }
}

fn build_envelope(params: &serde_json::Value, kind: Kind) -> Result<Envelope, RpcError> {
    let from = params
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(-32602, "missing from"))?
        .to_string();
    let to = params.get("to").and_then(|v| v.as_str()).map(String::from);
    let payload = params
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let timeout_ms = params.get("timeout_ms").and_then(|v| v.as_u64());
    Ok(Envelope {
        id: String::new(),
        kind,
        from,
        to,
        request_id: None,
        timeout_ms,
        ts: now_utc(),
        payload,
    })
}

fn err(code: i64, message: &str) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn map_route_err(e: RouteError) -> RpcError {
    match e {
        RouteError::UnknownInstance(id) => RpcError {
            code: 1003,
            message: "unknown_instance".into(),
            data: Some(json!({ "id": id })),
        },
        RouteError::AskTimeout { ms } => RpcError {
            code: 1005,
            message: "timeout".into(),
            data: Some(json!({ "timeout_ms": ms })),
        },
        RouteError::InstanceDisconnected(id) => RpcError {
            code: 1006,
            message: "instance_disconnected".into(),
            data: Some(json!({ "id": id })),
        },
        RouteError::UnknownRequestId(rid) => RpcError {
            code: 1007,
            message: "unknown_request_id".into(),
            data: Some(json!({ "request_id": rid })),
        },
        RouteError::Validation(v) => RpcError {
            code: -32602,
            message: v.to_string(),
            data: None,
        },
    }
}

pub fn make_response(
    req_id: serde_json::Value,
    res: Result<serde_json::Value, RpcError>,
) -> RpcResponse {
    match res {
        Ok(v) => RpcResponse {
            id: req_id,
            result: Some(v),
            error: None,
        },
        Err(e) => RpcResponse {
            id: req_id,
            result: None,
            error: Some(e),
        },
    }
}
