use std::{io::Cursor, path::Path};

use chrono::{DateTime, Duration, Utc};
use plist::Value;
use tokio::process::Command;

use super::{DevSyncError, models::SigningStatus};

const EXPIRING_SOON: Duration = Duration::hours(48);

/// Reads only non-secret provisioning metadata from an app produced by Xcode.
pub async fn inspect_embedded_profile(app: &Path) -> Result<SigningStatus, DevSyncError> {
    let profile = app.join("embedded.mobileprovision");
    if !profile.is_file() {
        return Ok(unknown_status());
    }
    let output = Command::new("security")
        .args(["cms", "-D", "-i"])
        .arg(&profile)
        .output()
        .await
        .map_err(|error| {
            DevSyncError::new(
                "profile_inspection_failed",
                format!("Unable to launch security: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(DevSyncError::new(
            "profile_inspection_failed",
            "macOS could not decode the embedded provisioning profile.",
        ));
    }
    parse_mobileprovision_at(&output.stdout, Utc::now())
}

#[cfg(test)]
pub fn parse_mobileprovision(bytes: &[u8]) -> Result<SigningStatus, DevSyncError> {
    parse_mobileprovision_at(bytes, Utc::now())
}

pub fn parse_mobileprovision_at(
    bytes: &[u8],
    inspected_at: DateTime<Utc>,
) -> Result<SigningStatus, DevSyncError> {
    let value = Value::from_reader_xml(Cursor::new(bytes)).map_err(|error| {
        DevSyncError::new(
            "profile_inspection_failed",
            format!("Invalid provisioning profile plist: {error}"),
        )
    })?;
    let dictionary = value.as_dictionary().ok_or_else(|| {
        DevSyncError::new(
            "profile_inspection_failed",
            "Provisioning profile is not a plist dictionary.",
        )
    })?;
    let expiration_date = dictionary
        .get("ExpirationDate")
        .and_then(Value::as_date)
        .map(|date| date.to_xml_format());
    let remaining_seconds = expiration_date
        .as_deref()
        .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
        .map(|date| (date.with_timezone(&Utc) - inspected_at).num_seconds());
    Ok(SigningStatus {
        team_identifier: dictionary
            .get("TeamIdentifier")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_string)
            .map(str::to_string),
        profile_uuid: dictionary
            .get("UUID")
            .and_then(Value::as_string)
            .map(str::to_string),
        application_identifier: dictionary
            .get("Entitlements")
            .and_then(Value::as_dictionary)
            .and_then(|values| values.get("application-identifier"))
            .and_then(Value::as_string)
            .map(str::to_string),
        profile_name: dictionary
            .get("Name")
            .and_then(Value::as_string)
            .map(str::to_string),
        expiration_date,
        remaining_seconds,
        last_inspected_at: inspected_at.to_rfc3339(),
        status: signing_state(remaining_seconds).into(),
    })
}

pub fn signing_state(remaining_seconds: Option<i64>) -> &'static str {
    match remaining_seconds {
        Some(seconds) if seconds <= 0 => "expired",
        Some(seconds) if seconds <= EXPIRING_SOON.num_seconds() => "expiringSoon",
        Some(_) => "valid",
        None => "unknown",
    }
}

/// Recomputes the derived countdown from the persisted profile expiration.
///
/// `remaining_seconds` is kept for backwards-compatible storage and UI
/// serialization, but it must never be treated as a durable clock: a value
/// captured during the build becomes stale as soon as time passes.
pub fn refresh_status(status: &mut SigningStatus) {
    let remaining_seconds = status
        .expiration_date
        .as_deref()
        .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
        .map(|date| (date.with_timezone(&Utc) - Utc::now()).num_seconds());
    status.remaining_seconds = remaining_seconds;
    status.status = signing_state(remaining_seconds).into();
}

pub fn has_usable_profile(status: &SigningStatus) -> bool {
    status.expiration_date.is_some()
        && status.remaining_seconds.is_some()
        && status.status != "unknown"
        && status.status != "expired"
}

pub fn is_sufficiently_renewed(
    status: &SigningStatus,
    previous_expiration: Option<&str>,
    threshold_seconds: i64,
) -> bool {
    if !has_usable_profile(status) {
        return false;
    }
    let Some(expiration) = status
        .expiration_date
        .as_deref()
        .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
    else {
        return false;
    };
    let now = Utc::now();
    if expiration.with_timezone(&Utc) <= now + Duration::seconds(threshold_seconds) {
        return false;
    }
    previous_expiration
        .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
        .map(|previous| expiration > previous)
        .unwrap_or(true)
}

fn unknown_status() -> SigningStatus {
    SigningStatus {
        team_identifier: None,
        profile_uuid: None,
        application_identifier: None,
        profile_name: None,
        expiration_date: None,
        remaining_seconds: None,
        last_inspected_at: Utc::now().to_rfc3339(),
        status: "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_profile_identity_metadata() {
        let plist = br#"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>ExpirationDate</key><date>2030-01-01T00:00:00Z</date><key>TeamIdentifier</key><array><string>TEAM</string></array><key>UUID</key><string>profile-id</string><key>Name</key><string>iOS Team Profile</string><key>Entitlements</key><dict><key>application-identifier</key><string>TEAM.me.example.app</string></dict></dict></plist>"#;
        let status = parse_mobileprovision(plist).unwrap();
        assert_eq!(status.team_identifier.as_deref(), Some("TEAM"));
        assert_eq!(
            status.application_identifier.as_deref(),
            Some("TEAM.me.example.app")
        );
        assert_eq!(status.status, "valid");
    }
    #[test]
    fn classifies_expiring_profiles() {
        assert_eq!(signing_state(Some(-1)), "expired");
        assert_eq!(signing_state(Some(60)), "expiringSoon");
        assert_eq!(signing_state(Some(60 * 60 * 72)), "valid");
        assert_eq!(signing_state(None), "unknown");
    }

    #[test]
    fn refreshes_a_persisted_countdown_from_expiration() {
        let mut status = status_for_test("2030-01-01T00:00:00Z", Some(1));
        refresh_status(&mut status);
        assert!(status.remaining_seconds.unwrap_or_default() > 0);
        assert_eq!(status.status, "valid");
    }

    #[test]
    fn requires_a_new_profile_to_outlive_the_refresh_threshold() {
        let mut status = status_for_test("2030-01-01T00:00:00Z", Some(1));
        refresh_status(&mut status);
        assert!(is_sufficiently_renewed(
            &status,
            Some("2029-01-01T00:00:00Z"),
            24 * 60 * 60
        ));
        assert!(!is_sufficiently_renewed(
            &status,
            Some("2030-01-01T00:00:00Z"),
            24 * 60 * 60
        ));
    }

    fn status_for_test(expiration_date: &str, remaining_seconds: Option<i64>) -> SigningStatus {
        SigningStatus {
            team_identifier: Some("TEAM".into()),
            profile_uuid: Some("profile".into()),
            application_identifier: Some("TEAM.me.example.app".into()),
            profile_name: Some("Profile".into()),
            expiration_date: Some(expiration_date.into()),
            remaining_seconds,
            last_inspected_at: "2026-01-01T00:00:00Z".into(),
            status: "valid".into(),
        }
    }
}
