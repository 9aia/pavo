//! Platform feed constructors — [`JobItem`] and
//! [`NotificationItem`], the payloads behind the `jobs` and
//! `notifications` wire ops.
//!
//! A driver becomes a feed *provider* by declaring the matching
//! `contributes` slot ([`crate::manifest::Manifest::jobs`] /
//! [`crate::manifest::Manifest::notifications`]); the ops then carry
//! item lists. Both are optional capabilities — drivers without a
//! feed answer `ok:false` (or `[]`), which hosts read as "no feed".
//!
//! ```
//! # use pavo_driver_kit as kit;
//! let j = kit::job("build-1", "Build pavo", "running");
//! let mut n = kit::notification("n-1", "Build finished");
//! n.severity = Some("info".to_string());
//! n.read = true;
//! ```

use serde_json::Map;

use pavo_protocol::{JobItem, NotificationItem};

/// One [`JobItem`] — `id`/`title`/`status` are the required trio
/// (`status` is a free-form driver-owned string: `"queued"`,
/// `"running"`, `"done"`, `"failed"`, …). `detail`, `route`,
/// `updated_at` (RFC 3339), and `extra` are public fields.
#[must_use]
pub fn job(id: impl Into<String>, title: impl Into<String>, status: impl Into<String>) -> JobItem {
    JobItem {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        detail: None,
        route: None,
        updated_at: None,
        extra: Map::new(),
    }
}

/// One [`NotificationItem`] — `id`/`title` required; `body`,
/// `severity` (free-form: `"info"`/`"warning"`/`"error"`/…),
/// `route`, `created_at` (RFC 3339), `read`, and `extra` are public
/// fields.
#[must_use]
pub fn notification(id: impl Into<String>, title: impl Into<String>) -> NotificationItem {
    NotificationItem {
        id: id.into(),
        title: title.into(),
        body: None,
        severity: None,
        route: None,
        created_at: None,
        read: false,
        extra: Map::new(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    /// Constructors emit the required fields; optionals stay absent.
    #[test]
    fn feed_items() {
        let j = job("j1", "Build", "running");
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["id"], json!("j1"));
        assert_eq!(v["status"], json!("running"));
        assert!(v.get("detail").is_none());

        let mut n = notification("n1", "Hi");
        n.severity = Some("warning".into());
        n.read = true;
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["severity"], json!("warning"));
        assert_eq!(v["read"], json!(true));
    }
}
