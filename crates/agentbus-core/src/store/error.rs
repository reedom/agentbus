use crate::envelope::ValidationError;

/// Spec section 8 error model. `code()` is the stable wire identifier.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("unknown instance `{0}` (recipients must register first; check list_instances / `agentbus ls`)")]
    UnknownInstance(String),
    #[error("instance_id `{0}` is registered to another live process (pick a different id; dead owners are replaced automatically)")]
    InstanceIdTaken(String),
    #[error("invalid instance_id (must match [A-Za-z0-9_.:-]{{1,128}})")]
    InvalidInstanceId,
    #[error("ask `{0}` timed out (a late reply stays retrievable via ask-result)")]
    Timeout(String),
    #[error("unknown request_id `{0}` (the request_id must be the ask envelope's id; plain `message` envelopes take no reply)")]
    UnknownRequestId(String),
    #[error("store locked: busy_timeout exhausted (transient write contention; retry after a short wait)")]
    StoreLocked,
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(#[from] ValidationError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(rusqlite::Error),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::UnknownInstance(_) => "unknown_instance",
            StoreError::InstanceIdTaken(_) => "instance_id_taken",
            StoreError::InvalidInstanceId => "invalid_instance_id",
            StoreError::Timeout(_) => "timeout",
            StoreError::UnknownRequestId(_) => "unknown_request_id",
            StoreError::StoreLocked => "store_locked",
            StoreError::InvalidEnvelope(_) => "invalid_envelope",
            StoreError::Io(_) => "io",
            StoreError::Sqlite(_) => "sqlite",
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(f, _) = &e {
            if f.code == rusqlite::ErrorCode::DatabaseBusy
                || f.code == rusqlite::ErrorCode::DatabaseLocked
            {
                return StoreError::StoreLocked;
            }
        }
        StoreError::Sqlite(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            StoreError::UnknownInstance("x".into()).code(),
            "unknown_instance"
        );
        assert_eq!(
            StoreError::InstanceIdTaken("x".into()).code(),
            "instance_id_taken"
        );
        assert_eq!(StoreError::InvalidInstanceId.code(), "invalid_instance_id");
        assert_eq!(StoreError::Timeout("x".into()).code(), "timeout");
        assert_eq!(
            StoreError::UnknownRequestId("x".into()).code(),
            "unknown_request_id"
        );
        assert_eq!(StoreError::StoreLocked.code(), "store_locked");
        assert_eq!(
            StoreError::InvalidEnvelope(crate::envelope::ValidationError::EmptyId).code(),
            "invalid_envelope"
        );
        assert_eq!(StoreError::Io(std::io::Error::other("x")).code(), "io");
        assert_eq!(
            StoreError::Sqlite(rusqlite::Error::InvalidQuery).code(),
            "sqlite"
        );
    }

    #[test]
    fn messages_carry_recovery_hints() {
        let unknown = StoreError::UnknownInstance("x".into()).to_string();
        assert!(unknown.contains("register"), "got: {unknown}");
        let taken = StoreError::InstanceIdTaken("x".into()).to_string();
        assert!(taken.contains("different id"), "got: {taken}");
        let no_ask = StoreError::UnknownRequestId("x".into()).to_string();
        assert!(no_ask.contains("ask envelope"), "got: {no_ask}");
        let locked = StoreError::StoreLocked.to_string();
        assert!(locked.contains("retry"), "got: {locked}");
        // Timeout already carries its hint; pin it so it never regresses.
        let timeout = StoreError::Timeout("x".into()).to_string();
        assert!(timeout.contains("ask-result"), "got: {timeout}");
    }
}
