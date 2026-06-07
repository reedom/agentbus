//! Initialize-time usage guidance for MCP clients (fr:16): the fallback
//! channel when no skill is teaching this client. The full text is a
//! condensed, hand-maintained subset of skills/agentbus/SKILL.md — when
//! that skill's mental model or gotchas change, re-derive this text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    Minimal,
    Full,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "none" => Some(Level::None),
            "minimal" => Some(Level::Minimal),
            "full" => Some(Level::Full),
            _ => None,
        }
    }
}

pub fn text(level: Level) -> Option<&'static str> {
    match level {
        Level::None => None,
        Level::Minimal => Some(MINIMAL),
        Level::Full => Some(FULL),
    }
}

const MINIMAL: &str = "\
agentbus message bus. register(instance_id) first; send/ask to talk; \
check_inbox or await_message to receive (batch; empty = timeout). Answer \
an inbound ask with reply(request_id = the ask envelope's id; omit `to`). \
Never ask your own id (deadlock).";

const FULL: &str = "\
agentbus is a daemonless message bus over a shared local store \
(~/.agentbus). Envelope kinds: message (one-way), ask (blocks for a \
reply), reply (resolves an ask), event (broadcast log).

Quickstart:
1. register(instance_id) first. Only recipients need registration; any \
`from` string may send.
2. send / ask / publish_event to talk. check_inbox (non-blocking) or \
await_message (blocking) to receive; both return {\"envelopes\": [...]}. \
An empty await_message batch means timeout — a normal outcome, not an \
error.
3. To answer an inbound ask: reply(from=<you>, request_id=<the ask \
envelope's id>, payload=...). Do not set `to`; the store routes it.

Rules:
- Never ask your own instance_id: you would block waiting for a reply \
only you could write (deadlock).
- payload is structured JSON (object/array/number), not a stringified \
blob.
- An ask timeout does not discard the request; the error data carries \
the request_id and a late reply stays retrievable (CLI: agentbus \
ask-result <request_id>).
- Registrations default to dying with this session; persistent=true \
survives until unregister.
- Errors arrive as {\"message\": <stable code>, \"data\": <detail with \
a recovery hint>}.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_the_three_levels_and_rejects_junk() {
        assert_eq!(Level::parse("none"), Some(Level::None));
        assert_eq!(Level::parse("minimal"), Some(Level::Minimal));
        assert_eq!(Level::parse("full"), Some(Level::Full));
        assert_eq!(Level::parse("FULL"), None);
        assert_eq!(Level::parse(""), None);
    }

    #[test]
    fn none_yields_no_text_and_others_teach_the_reply_rule() {
        assert!(text(Level::None).is_none());
        for level in [Level::Minimal, Level::Full] {
            let t = text(level).unwrap();
            assert!(t.contains("ask envelope's id"), "{level:?}: {t}");
        }
    }
}
