---
refs:
  id: fr:06-rest-api
  kind: fr
  title: "REST API surface"
  related:
    - ref:protocol
    - fr:01-envelope
    - fr:02-instance-registry
    - fr:04-router
    - fr:07-sse
  modules:
    - crates/agentbusd/src/http
---

# FR 06: REST API surface

> The loopback-bound HTTP API that lets external programs and the CLI use the bus.

## Purpose

The REST API is the bus's surface for everything that is not an MCP client —
scripts, CI, webhooks, bridges, and the `agentbus` CLI. It exposes registration,
messaging, asking, replying, instance listing, and event publishing over plain
HTTP so any HTTP-capable program can participate.

## User-visible Behavior

All endpoints are versioned under `/v1` and bound to `127.0.0.1`:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/instances` | Register `{instance_id, mailbox_size?}` |
| `DELETE` | `/v1/instances/{id}` | Unregister |
| `GET` | `/v1/instances` | List active instances |
| `GET` | `/v1/instances/{id}/inbox` | SSE — envelopes addressed to `{id}` |
| `POST` | `/v1/instances/{id}/messages` | Send a `message` to `{id}` |
| `POST` | `/v1/instances/{id}/ask` | Ask `{id}`; blocks until reply or timeout |
| `POST` | `/v1/instances/{id}/replies` | Reply to an ask `{request_id, payload}` |
| `GET` | `/v1/events` | SSE — global broadcast + history replay |
| `POST` | `/v1/events` | Publish a broadcast event |

- `POST /v1/instances` binds the registration to the HTTP connection's
  lifetime: the response is `200 OK` with `Connection: keep-alive` and an
  SSE-style heartbeat body. Closing the connection unregisters. An explicit
  `DELETE /v1/instances/{id}` is also supported for clients using pooled
  connections.
- `POST /v1/instances/{id}/ask` accepts `?timeout_ms=` (default `30_000`, max
  24h) and holds the HTTP connection open until a reply or timeout.
- `GET /v1/events` accepts `since=<ts>`, `instance=<id>`, and `kind=<kind>`
  query filters and is served by fr:07-sse.

## Capabilities

- Full bus participation over plain HTTP for non-MCP programs.
- Connection-bound registration with an explicit-DELETE fallback.
- Blocking `ask` over HTTP — the request returns the reply payload directly.
- External event publishing via `POST /v1/events`.
- A stable versioned (`/v1`) path namespace.

## Boundaries

- No authentication, authorization, or TLS — the API is loopback-only by
  design (spec §1.1, §8.10); auth is post-v1 (§13).
- The exact registration-binding mechanism (keep-alive long-poll vs a
  registration token) is an open question pending an `axum` connection-lifecycle
  spike (spec §12).
- The API does not interpret `payload`; it only frames envelopes.
- SSE streaming semantics (replay, slow-subscriber handling) belong to
  fr:07-sse, not this FR.
- Registry and routing logic are delegated to fr:02-instance-registry and
  fr:04-router; this FR owns only the HTTP surface.

## Error Handling

- `ask` timeout: `POST /v1/instances/{id}/ask` returns `504` with body
  `{error: "timeout", request_id}` when `timeout_ms` elapses (spec §7.2,
  §8.5; owned by fr:04-router).
- Inbox ownership: `GET /v1/instances/{id}/inbox` requires the caller to be the
  registered owner, matched by connection; a different connection requesting
  another instance's inbox returns `403` (spec §6.5).
- Registration collision surfaces the registry's `instance_id_taken` (spec
  §8.2, fr:02-instance-registry).

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:01-envelope, fr:02-instance-registry, fr:04-router, fr:07-sse

## When to update

- An endpoint is added, removed, or its path changes.
- The API version prefix changes.
- The registration-binding mechanism is finalized or changed.
- The `ask` timeout query parameter or default changes.
