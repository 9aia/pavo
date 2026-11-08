//! Contribution merge — fold every discovered driver's
//! [`pavo_protocol::Contributes`] into one [`Chrome`] description the
//! renderers lay out.
//!
//! ## Ordering policy (deterministic)
//!
//! Within each slot, entries sort by **`priority` ascending — lower
//! wins** — per `pavo_protocol::Contribution`'s contract ("lower
//! sorts first"). Ties break by **driver kind ascending**, then
//! **contribution id ascending**. The protocol doc says ties break by
//! "driver discovery order"; we substitute driver name because PATH
//! order is environment-dependent and nondeterministic across
//! machines — name order is stable and gives the same "one winner"
//! guarantee. Same-slot duplicate ids from *different* drivers can't
//! collide: every id is namespaced `<driver>:<id>` at merge time.
//!
//! `extra` slots (unknown to this host version) merge the same way
//! and pass through untouched — forward compatibility per the
//! protocol's degrade-gracefully rule.

use std::collections::BTreeMap;

use pavo_protocol::{Contribution, DriverManifest};

/// One slot entry after merging — the contribution plus which driver
/// it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotEntry {
    /// Driver kind (`<type>` of `tamias-driver-<type>`).
    pub driver: String,
    /// Driver display title (manifest `title`).
    pub driver_title: String,
    /// The contribution, with `id` already namespaced as
    /// `<driver>:<id>`.
    pub contribution: Contribution,
}

/// The composed chrome — every slot merged across all drivers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chrome {
    /// Top navigation bar entries.
    pub navbar: Vec<SlotEntry>,
    /// Shared dashboard panels.
    pub dashboard: Vec<SlotEntry>,
    /// Bottom status bar entries.
    pub statusbar: Vec<SlotEntry>,
    /// Standalone routes drivers own outside their views.
    pub routes: Vec<SlotEntry>,
    /// Unknown/forward-compat slots, merged the same way, keyed by
    /// slot name. Renderers ignore slots they don't know.
    pub extra: BTreeMap<String, Vec<SlotEntry>>,
}

/// Merge `drivers` — `(kind, manifest)` pairs — into a [`Chrome`].
///
/// Drivers with no `contributes` contribute nothing; empty slots stay
/// empty. Ordering per the module docs.
#[must_use]
pub fn merge_chrome<'a, I>(drivers: I) -> Chrome
where
    I: IntoIterator<Item = (&'a str, &'a DriverManifest)>,
{
    let mut chrome = Chrome::default();
    for (kind, manifest) in drivers {
        let title = manifest.title.as_str();
        collect(
            &mut chrome.navbar,
            kind,
            title,
            &manifest.contributes.navbar,
        );
        collect(
            &mut chrome.dashboard,
            kind,
            title,
            &manifest.contributes.dashboard,
        );
        collect(
            &mut chrome.statusbar,
            kind,
            title,
            &manifest.contributes.statusbar,
        );
        collect(
            &mut chrome.routes,
            kind,
            title,
            &manifest.contributes.routes,
        );
        for (slot, entries) in &manifest.contributes.extra {
            let target = chrome.extra.entry(slot.clone()).or_default();
            for c in entries {
                target.push(slot_entry(kind, title, c));
            }
        }
    }
    sort_slot(&mut chrome.navbar);
    sort_slot(&mut chrome.dashboard);
    sort_slot(&mut chrome.statusbar);
    sort_slot(&mut chrome.routes);
    for entries in chrome.extra.values_mut() {
        sort_slot(entries);
    }
    chrome
}

/// Append one driver's contributions to a slot, namespacing ids.
fn collect(out: &mut Vec<SlotEntry>, kind: &str, title: &str, entries: &[Contribution]) {
    for c in entries {
        out.push(slot_entry(kind, title, c));
    }
}

/// Build a [`SlotEntry`] with the id namespaced `<driver>:<id>`.
fn slot_entry(kind: &str, title: &str, c: &Contribution) -> SlotEntry {
    let mut contribution = c.clone();
    contribution.id = format!("{kind}:{}", c.id);
    SlotEntry {
        driver: kind.to_string(),
        driver_title: title.to_string(),
        contribution,
    }
}

/// The deterministic order: priority asc (lower wins), driver kind
/// asc, id asc.
fn sort_slot(entries: &mut [SlotEntry]) {
    entries.sort_by(|a, b| {
        a.contribution
            .priority
            .cmp(&b.contribution.priority)
            .then_with(|| a.driver.cmp(&b.driver))
            .then_with(|| a.contribution.id.cmp(&b.contribution.id))
    });
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::Contributes;

    fn manifest(name: &str, title: &str, contributes: Contributes) -> DriverManifest {
        DriverManifest {
            name: name.to_string(),
            title: title.to_string(),
            icon: None,
            version: "0.0.0".to_string(),
            verbs: vec!["serve".to_string()],
            capabilities: vec!["pavo".to_string()],
            contributes,
            views: vec![],
            renderers: vec![],
        }
    }

    fn contrib(id: &str, priority: i32) -> Contribution {
        Contribution {
            id: id.to_string(),
            title: id.to_string(),
            route: format!("/{id}"),
            priority,
            icon: None,
        }
    }

    #[test]
    fn merges_and_sorts_by_priority_then_driver() {
        let a = manifest(
            "aaa",
            "A",
            Contributes {
                navbar: vec![contrib("late", 50), contrib("first", 10)],
                ..Contributes::default()
            },
        );
        let b = manifest(
            "bbb",
            "B",
            Contributes {
                navbar: vec![contrib("mid", 10)],
                ..Contributes::default()
            },
        );
        let chrome = merge_chrome([("aaa", &a), ("bbb", &b)]);
        let ids: Vec<&str> = chrome
            .navbar
            .iter()
            .map(|e| e.contribution.id.as_str())
            .collect();
        // priority 10 tie → driver name asc: aaa:mid? no — aaa has
        // "first"@10 and "late"@50; bbb has "mid"@10. Order: the two
        // priority-10 entries sort by driver: aaa:first, bbb:mid, then
        // aaa:late.
        assert_eq!(ids, ["aaa:first", "bbb:mid", "aaa:late"]);
        assert_eq!(chrome.navbar[0].driver_title, "A");
    }

    #[test]
    fn namespaces_ids_and_routes_statusbar_dashboard() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("weird".to_string(), vec![contrib("w", 1)]);
        let m = manifest(
            "drv",
            "Drv",
            Contributes {
                dashboard: vec![contrib("dash", 5)],
                statusbar: vec![contrib("st", 1)],
                routes: vec![contrib("r", 1)],
                extra,
                ..Contributes::default()
            },
        );
        let chrome = merge_chrome([("drv", &m)]);
        assert_eq!(chrome.dashboard[0].contribution.id, "drv:dash");
        assert_eq!(chrome.statusbar[0].contribution.id, "drv:st");
        assert_eq!(chrome.routes[0].contribution.id, "drv:r");
        assert_eq!(
            chrome.extra["weird"][0].contribution.id, "drv:w",
            "extra slots merge + namespace too"
        );
    }

    #[test]
    fn empty_inputs_give_empty_chrome() {
        let chrome = merge_chrome(std::iter::empty::<(&str, &DriverManifest)>());
        assert!(chrome.navbar.is_empty() && chrome.extra.is_empty());
    }
}
