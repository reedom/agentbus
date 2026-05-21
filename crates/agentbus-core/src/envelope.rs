//! Canonical wire envelope.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Message,
    Ask,
    Reply,
    Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub kind: Kind,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
    pub payload: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("ask envelopes require `to`")]
    AskMissingTo,
    #[error("reply envelopes require `to` and `request_id`")]
    ReplyMissingFields,
    #[error("message envelopes require `to`")]
    MessageMissingTo,
    #[error("event envelopes must not have `to`")]
    EventHasTo,
    #[error("`from` is empty")]
    EmptyFrom,
    #[error("`id` is empty")]
    EmptyId,
}

impl Envelope {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.is_empty() { return Err(ValidationError::EmptyId); }
        if self.from.is_empty() { return Err(ValidationError::EmptyFrom); }
        match self.kind {
            Kind::Ask => {
                if self.to.is_none() { return Err(ValidationError::AskMissingTo); }
            }
            Kind::Reply => {
                if self.to.is_none() || self.request_id.is_none() {
                    return Err(ValidationError::ReplyMissingFields);
                }
            }
            Kind::Message => {
                if self.to.is_none() { return Err(ValidationError::MessageMissingTo); }
            }
            Kind::Event => {
                if self.to.is_some() { return Err(ValidationError::EventHasTo); }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn sample(kind: Kind) -> Envelope {
        Envelope {
            id: "msg_1".into(),
            kind,
            from: "alice".into(),
            to: Some("bob".into()),
            request_id: None,
            timeout_ms: None,
            ts: datetime!(2026-05-21 08:00:00 UTC),
            payload: serde_json::json!({"hello": "world"}),
        }
    }

    #[test]
    fn roundtrip_message() {
        let env = sample(Kind::Message);
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn validation_message_requires_to() {
        let mut env = sample(Kind::Message);
        env.to = None;
        assert!(matches!(env.validate(), Err(ValidationError::MessageMissingTo)));
    }

    #[test]
    fn validation_event_rejects_to() {
        let env = sample(Kind::Event);
        // `to` is Some — invalid for event
        assert!(matches!(env.validate(), Err(ValidationError::EventHasTo)));
    }

    #[test]
    fn validation_reply_requires_request_id() {
        let mut env = sample(Kind::Reply);
        env.request_id = None;
        assert!(matches!(env.validate(), Err(ValidationError::ReplyMissingFields)));
    }
}
