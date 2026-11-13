//! RFC 3339 timestamps on the Admin API wire.
//!
//! `time::OffsetDateTime`'s default serde representation is a nine-element array
//! (`[year, ordinal, hour, minute, second, nanosecond, offset_hours, …]`) — not a
//! timestamp a JavaScript client can parse. The Admin UI declares these fields as
//! strings and renders them with `new Date(value)`, so an array silently became an
//! empty cell, and a chart axis built from `Date.parse(hour)` sorted by `NaN`.
//!
//! Every Admin API timestamp therefore serializes as an RFC 3339 string in UTC,
//! e.g. `2026-11-13T03:23:10.304486932Z`. This module is response-only: no Admin
//! request DTO accepts a timestamp, so nothing downstream depends on the array form.

use serde::Serializer;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn to_rfc3339(value: &OffsetDateTime) -> Result<String, time::error::Format> {
    value.to_offset(time::UtcOffset::UTC).format(&Rfc3339)
}

/// Serializes a required timestamp as an RFC 3339 string.
pub fn serialize<S: Serializer>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    let formatted = to_rfc3339(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&formatted)
}

/// Serializes an optional timestamp as an RFC 3339 string or `null`.
pub mod option {
    use super::{OffsetDateTime, Serializer, to_rfc3339};

    pub fn serialize<S: Serializer>(
        value: &Option<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => {
                let formatted = to_rfc3339(value).map_err(serde::ser::Error::custom)?;
                serializer.serialize_some(&formatted)
            }
            None => serializer.serialize_none(),
        }
    }
}

/// Convenience for tests and call sites that build a DTO by hand.
pub fn format(value: OffsetDateTime) -> String {
    to_rfc3339(&value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use serde::Serialize;
    use time::macros::datetime;

    #[derive(Serialize)]
    struct Row {
        #[serde(with = "crate::admin_time")]
        at: OffsetDateTime,
        #[serde(with = "crate::admin_time::option")]
        maybe: Option<OffsetDateTime>,
    }

    #[test]
    fn timestamps_serialize_as_parseable_rfc3339_strings() {
        let value = datetime!(2026-11-13 03:23:10.123456789 UTC);
        let json = serde_json::to_value(Row {
            at: value,
            maybe: Some(value),
        })
        .expect("serialize");
        assert_eq!(json["at"], "2026-11-13T03:23:10.123456789Z");
        assert_eq!(json["maybe"], "2026-11-13T03:23:10.123456789Z");

        let json = serde_json::to_value(Row {
            at: value,
            maybe: None,
        })
        .expect("serialize");
        assert!(json["maybe"].is_null());
    }

    #[test]
    fn non_utc_offsets_are_normalized_to_utc() {
        let value = datetime!(2026-11-13 03:23:10 +02:00);
        assert_eq!(format(value), "2026-11-13T01:23:10Z");
    }
}
