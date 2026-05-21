//! ULID-based identifiers and RFC3339 timestamps.

use time::OffsetDateTime;
use ulid::Ulid;

pub fn new_envelope_id() -> String {
    format!("msg_{}", Ulid::new())
}

pub fn now_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

pub fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_has_prefix_and_is_unique() {
        let a = new_envelope_id();
        let b = new_envelope_id();
        assert!(a.starts_with("msg_"));
        assert_ne!(a, b);
    }

    #[test]
    fn now_is_utc() {
        let t = now_utc();
        assert_eq!(t.offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn parse_known_timestamp() {
        let t = parse_rfc3339("2026-05-21T08:00:00Z").unwrap();
        assert_eq!(t.year(), 2026);
    }
}
