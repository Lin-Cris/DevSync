use super::models::SigningStatus;

/// Keep the renewal boundary centralized for the scheduler and UI semantics.
pub const DEFAULT_REFRESH_THRESHOLD_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDecision {
    NoAction,
    RefreshNeeded,
    RefreshUrgently,
    InspectRequired,
}

#[derive(Debug, Clone, Copy)]
pub struct RefreshPolicy {
    pub refresh_threshold_seconds: i64,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            refresh_threshold_seconds: DEFAULT_REFRESH_THRESHOLD_SECONDS,
        }
    }
}

impl RefreshPolicy {
    pub fn evaluate(&self, signing: Option<&SigningStatus>) -> RefreshDecision {
        let Some(signing) = signing else {
            return RefreshDecision::InspectRequired;
        };
        match signing.remaining_seconds {
            None => RefreshDecision::InspectRequired,
            Some(seconds) if seconds <= 0 => RefreshDecision::RefreshUrgently,
            Some(seconds) if seconds <= self.refresh_threshold_seconds => {
                RefreshDecision::RefreshNeeded
            }
            Some(_) => RefreshDecision::NoAction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devsync::models::SigningStatus;

    fn status(remaining_seconds: Option<i64>) -> SigningStatus {
        SigningStatus {
            team_identifier: None,
            profile_uuid: None,
            application_identifier: None,
            profile_name: None,
            expiration_date: None,
            remaining_seconds,
            last_inspected_at: "2026-01-01T00:00:00Z".into(),
            status: "valid".into(),
        }
    }

    #[test]
    fn uses_the_configured_24_hour_refresh_boundary() {
        let policy = RefreshPolicy::default();
        assert_eq!(
            policy.evaluate(Some(&status(Some(7 * 24 * 60 * 60)))),
            RefreshDecision::NoAction
        );
        assert_eq!(
            policy.evaluate(Some(&status(Some(2 * 24 * 60 * 60)))),
            RefreshDecision::NoAction
        );
        assert_eq!(
            policy.evaluate(Some(&status(Some(24 * 60 * 60 + 60)))),
            RefreshDecision::NoAction
        );
        assert_eq!(
            policy.evaluate(Some(&status(Some(24 * 60 * 60)))),
            RefreshDecision::RefreshNeeded
        );
        assert_eq!(
            policy.evaluate(Some(&status(Some(24 * 60 * 60 - 1)))),
            RefreshDecision::RefreshNeeded
        );
        assert_eq!(
            policy.evaluate(Some(&status(Some(-1)))),
            RefreshDecision::RefreshUrgently
        );
        assert_eq!(policy.evaluate(None), RefreshDecision::InspectRequired);
    }
}
