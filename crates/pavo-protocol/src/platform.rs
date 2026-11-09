//! Platform feeds — the item shapes behind the `jobs` and
//! `notifications` wire ops.
//!
//! Drivers that declare the matching `contributes` slots
//! ([`crate::manifest::Contributes::jobs`] /
//! [`crate::manifest::Contributes::notifications`]) are *providers*:
//! the manifest entries are display metadata (label, icon, ordering)
//! and the item data itself flows over the wire —
//!
//! ```json
//! {"op":"jobs"}          → Vec<JobItem>
//! {"op":"notifications"}→ Vec<NotificationItem>
//! ```
//!
//! Both are optional capabilities: drivers without the feed answer
//! `ok:false` (or an empty list) per the existing wire convention —
//! hosts must tolerate either. The host knows which driver produced
//! an item, so neither type carries driver attribution.
//!
//! Timestamps are **RFC 3339 strings** (e.g. `"2026-01-02T15:04:05Z"`)
//! — chosen over unix epochs so payloads stay human-readable in
//! transcripts and `--json` dumps. `status`/`severity` are free-form
//! strings: drivers own their vocabularies (`"queued"`, `"running"`,
//! `"failed"`; `"info"`, `"warning"`, `"error"`, …) and hosts render
//! them verbatim or degrade to neutral styling.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One job/queue entry from a driver's `jobs` op — a unit of
/// background or queued work the host can list and deep-link into.
///
/// Wire shape:
///
/// ```json
/// {"id":"build-42","title":"Build pavo","status":"running",
///  "detail":"step 3/7","route":"/myco/job/build-42",
///  "updated_at":"2026-01-02T15:04:05Z"}
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobItem {
    /// Stable id within the driver — dedup key for hosts merging
    /// feeds across drivers (namespaced `<driver>:<id>` on the host
    /// side, same convention as [`crate::manifest::Contribution`]).
    pub id: String,
    /// Display label for the job.
    pub title: String,
    /// Free-form status string owned by the driver (`"queued"`,
    /// `"running"`, `"done"`, `"failed"`, …). Required but may be
    /// empty — hosts must not assume a fixed vocabulary.
    pub status: String,
    /// Optional longer description — progress note, failure reason,
    /// current step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional route/url the host navigates to on "open" — same
    /// resolution rules as [`crate::manifest::Contribution::route`]
    /// (driver-relative view routes or host-absolute paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Optional last-updated timestamp — RFC 3339 string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Forward-compat catch-all — unknown fields a future driver
    /// emits survive a typed round-trip.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One notification from a driver's `notifications` op — a discrete
/// alert/event the host can badge, list, and deep-link into.
///
/// Wire shape:
///
/// ```json
/// {"id":"n-7","title":"Build failed","severity":"error",
///  "body":"cargo xtask check exited 1","route":"/myco/job/build-42",
///  "created_at":"2026-01-02T15:04:05Z","read":false}
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NotificationItem {
    /// Stable id within the driver — dedup key for hosts merging
    /// feeds across drivers.
    pub id: String,
    /// Display title — the one-line summary.
    pub title: String,
    /// Optional body — longer detail shown expanded or on the
    /// notification's own surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional free-form severity string owned by the driver
    /// (`"info"`, `"warning"`, `"error"`, …). Hosts style verbatim
    /// or degrade to neutral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Optional route/url for "open" navigation — same resolution
    /// rules as [`JobItem::route`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Optional creation timestamp — RFC 3339 string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Whether the notification has been read. `false` (the default)
    /// typically drives unread badges.
    #[serde(default)]
    pub read: bool,
    /// Forward-compat catch-all — unknown fields a future driver
    /// emits survive a typed round-trip.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    /// Both item types round-trip and tolerate unknown fields via
    /// `extra`.
    #[test]
    fn platform_items_roundtrip_and_preserve_extras() {
        let job = JobItem {
            id: "j1".into(),
            title: "Build".into(),
            status: "running".into(),
            detail: Some("step 2/5".into()),
            route: Some("/myco/job/j1".into()),
            updated_at: Some("2026-01-02T15:04:05Z".into()),
            extra: Map::from_iter([("progress".to_string(), json!(40))]),
        };
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v["id"], "j1");
        assert_eq!(v["progress"], json!(40)); // extra flattens out
        let back: JobItem = serde_json::from_value(v).unwrap();
        assert_eq!(back, job);

        // Sparse wire shape: only the required trio.
        let n: NotificationItem =
            serde_json::from_value(json!({"id":"n1","title":"Hi","future":true})).unwrap();
        assert_eq!(n.id, "n1");
        assert!(!n.read);
        assert_eq!(n.extra["future"], json!(true));

        let n2 = NotificationItem {
            id: "n2".into(),
            title: "Done".into(),
            body: None,
            severity: Some("info".into()),
            route: None,
            created_at: Some("2026-01-02T15:04:05Z".into()),
            read: true,
            extra: Map::new(),
        };
        let v2 = serde_json::to_value(&n2).unwrap();
        assert_eq!(v2["read"], json!(true));
        assert!(v2.get("body").is_none()); // absent, not null
        let back2: NotificationItem = serde_json::from_value(v2).unwrap();
        assert_eq!(back2, n2);
    }
}
