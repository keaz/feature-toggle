use chrono::{DateTime, Utc};
use tracing::warn;

pub fn parse_rollback_timestamp(ts: &str) -> Option<DateTime<Utc>> {
    if ts.trim().is_empty() {
        return None;
    }
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => Some(dt.with_timezone(&Utc)),
        Err(err) => {
            warn!("Failed to parse rollback_scheduled_at '{}': {}", ts, err);
            None
        }
    }
}

pub fn is_scheduled_disable_due(ts: &str) -> bool {
    parse_rollback_timestamp(ts).map_or(false, |dt| dt <= Utc::now())
}
