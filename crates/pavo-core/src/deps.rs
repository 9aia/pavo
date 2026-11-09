//! Driver dependency ordering — topological sort over the
//! [`pavo_protocol::DriverDeps`] declarations manifests carry.
//!
//! A driver's `deps.requires` names drivers it cannot function
//! without; `deps.optional` names drivers it uses when present.
//! Both kinds of edge mean the same thing for ordering: **the dep
//! sorts before the dependent**. The differences are in validation:
//!
//! - `requires` naming a driver that isn't discovered is a hard
//!   error ([`DepsError::Missing`]) — the dependent can't function,
//!   so the host quarantines it (see [`crate::host::Host`]).
//! - `optional` naming an absent driver is fine — no edge, no error.
//! - Cycles — including a driver requiring itself — are a hard error
//!   ([`DepsError::Cycle`]).
//!
//! Ordering is deterministic: among drivers whose deps are all
//! satisfied, the alphabetically smallest kind is emitted next —
//! dep order first, driver name ascending as the tie-break (the same
//! substitution chrome merge makes for discovery order).
//!
//! Drivers whose manifests aren't known yet (`deps: None` — the
//! lazy-manifest case) contribute no edges and sort unconstrained;
//! [`crate::host::Host`] validates their `requires` when the manifest
//! arrives over the wire.

use std::collections::{BTreeMap, BTreeSet};

use pavo_protocol::DriverDeps;

/// Why dependency ordering failed — carries enough structure for the
/// host to quarantine exactly the offending drivers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepsError {
    /// `driver` declares `requires: [dep]` but no discovered driver
    /// has that kind — the dependent can't function without it.
    Missing {
        /// The driver declaring the unmet requirement.
        driver: String,
        /// The required driver name that isn't discovered.
        dep: String,
    },
    /// The dep graph couldn't be ordered — `drivers` are the kinds
    /// stuck in the cycle (sorted, so output is deterministic).
    Cycle {
        /// Driver kinds participating in the cycle.
        drivers: Vec<String>,
    },
}

impl DepsError {
    /// The driver kinds responsible for the failure — what a host
    /// quarantines. `Missing` blames the *dependent* (it declared
    /// the requirement); `Cycle` blames every member.
    #[must_use]
    pub fn offenders(&self) -> Vec<String> {
        match self {
            Self::Missing { driver, .. } => vec![driver.clone()],
            Self::Cycle { drivers } => drivers.clone(),
        }
    }
}

impl std::fmt::Display for DepsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { driver, dep } => {
                write!(f, "driver '{driver}' requires missing driver '{dep}'")
            }
            Self::Cycle { drivers } => {
                write!(f, "driver dependency cycle: {}", drivers.join(" -> "))
            }
        }
    }
}

impl std::error::Error for DepsError {}

/// Order driver kinds so every dep precedes its dependents.
///
/// `drivers` yields `(kind, deps)` pairs — `deps` is `None` when the
/// driver's manifest isn't known yet (lazy manifest); such drivers
/// are unconstrained. Duplicate kinds keep the first occurrence's
/// deps (discovery already dedups by kind; this is just defensive).
///
/// # Errors
///
/// [`DepsError::Missing`] when a `requires` name isn't among the
/// input kinds; [`DepsError::Cycle`] when the graph can't be
/// topologically ordered.
pub fn order<'a, I>(drivers: I) -> Result<Vec<String>, DepsError>
where
    I: IntoIterator<Item = (&'a str, Option<&'a DriverDeps>)>,
{
    // Collect first — the iterator is single-pass. kind → deps,
    // first occurrence wins on duplicates.
    let mut deps_of: BTreeMap<String, DriverDeps> = BTreeMap::new();
    for (kind, deps) in drivers {
        deps_of
            .entry(kind.to_string())
            .or_insert_with(|| deps.cloned().unwrap_or_default());
    }

    // `requires` must name a discovered driver — checked before any
    // ordering so the error names the offender, not a symptom.
    // (Kinds are checked against `deps_of`, which holds every input
    // driver regardless of dep knowledge.)
    let kinds: BTreeSet<&str> = deps_of.keys().map(String::as_str).collect();
    for (kind, deps) in &deps_of {
        for req in &deps.requires {
            if !kinds.contains(req.as_str()) {
                return Err(DepsError::Missing {
                    driver: kind.clone(),
                    dep: req.clone(),
                });
            }
        }
    }

    // Edges: dep → dependent, for every dep name (requires or
    // optional) that is itself a discovered driver. Absent optional
    // deps contribute nothing; absent requires already errored.
    // A self-edge is kept — Kahn's can't emit it, so it surfaces as
    // a cycle.
    let mut dependents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new(); // dep → drivers waiting on it
    let mut pending: BTreeMap<&str, usize> = BTreeMap::new(); // driver → unmet dep count
    for (kind, deps) in &deps_of {
        let mut unmet = BTreeSet::new();
        for name in deps.requires.iter().chain(&deps.optional) {
            if kinds.contains(name.as_str()) {
                unmet.insert(name.as_str());
            }
        }
        pending.insert(kind.as_str(), unmet.len());
        for name in unmet {
            dependents.entry(name).or_default().insert(kind.as_str());
        }
    }

    // Kahn's with a name-asc tie-break: repeatedly emit the
    // alphabetically smallest driver whose deps are all emitted.
    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    let mut order = Vec::with_capacity(deps_of.len());
    loop {
        let next = pending
            .iter()
            .filter(|(kind, count)| **count == 0 && !emitted.contains(*kind))
            .map(|(kind, _)| *kind)
            .min();
        let Some(kind) = next else {
            break;
        };
        emitted.insert(kind);
        order.push(kind.to_string());
        if let Some(waiting) = dependents.get(kind) {
            for dep in waiting {
                if let Some(count) = pending.get_mut(dep) {
                    *count -= 1;
                }
            }
        }
    }

    if order.len() != deps_of.len() {
        // Whatever couldn't be emitted is stuck behind a cycle.
        let stuck: Vec<String> = pending
            .iter()
            .filter(|(kind, _)| !emitted.contains(*kind))
            .map(|(kind, _)| kind.to_string())
            .collect();
        return Err(DepsError::Cycle { drivers: stuck });
    }
    Ok(order)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn deps(requires: &[&str], optional: &[&str]) -> DriverDeps {
        DriverDeps {
            requires: requires.iter().map(|s| s.to_string()).collect(),
            optional: optional.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Helper: order a slice of (kind, deps) — deps owned locally so
    /// the borrows line up.
    fn order_owned(items: &[(&str, DriverDeps)]) -> Result<Vec<String>, DepsError> {
        let refs: Vec<(&str, Option<&DriverDeps>)> =
            items.iter().map(|(k, d)| (*k, Some(d))).collect();
        order(refs)
    }

    #[test]
    fn dep_sorts_before_dependent() {
        // "zzz" requires "aaa" — despite name order, aaa comes first.
        let items = [
            ("zzz", deps(&["aaa"], &[])),
            ("aaa", deps(&[], &[])),
            ("mmm", deps(&[], &["aaa"])), // optional edge too
        ];
        let got = order_owned(&items).unwrap();
        assert_eq!(got, ["aaa", "mmm", "zzz"]);
    }

    #[test]
    fn absent_optional_is_fine_absent_requires_errors() {
        let items = [("a", deps(&[], &["ghost"])), ("b", deps(&[], &[]))];
        assert_eq!(order_owned(&items).unwrap(), ["a", "b"]);

        let items = [("a", deps(&["ghost"], &[])), ("b", deps(&[], &[]))];
        let err = order_owned(&items).unwrap_err();
        assert_eq!(
            err,
            DepsError::Missing {
                driver: "a".into(),
                dep: "ghost".into()
            }
        );
        assert_eq!(err.offenders(), ["a"]);
    }

    #[test]
    fn cycles_error_with_members() {
        let items = [
            ("a", deps(&["b"], &[])),
            ("b", deps(&["a"], &[])),
            ("c", deps(&[], &[])),
        ];
        let err = order_owned(&items).unwrap_err();
        assert_eq!(
            err,
            DepsError::Cycle {
                drivers: vec!["a".into(), "b".into()]
            }
        );
        assert_eq!(err.offenders(), ["a", "b"]);
    }

    #[test]
    fn self_dependency_is_a_cycle() {
        let items = [("a", deps(&["a"], &[]))];
        let err = order_owned(&items).unwrap_err();
        assert!(matches!(err, DepsError::Cycle { .. }));
    }

    #[test]
    fn no_deps_keeps_name_order() {
        let items = [
            ("ccc", deps(&[], &[])),
            ("aaa", deps(&[], &[])),
            ("bbb", deps(&[], &[])),
        ];
        assert_eq!(order_owned(&items).unwrap(), ["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn diamond_orders_transitively() {
        // d needs b+c; b,c need a.
        let items = [
            ("d", deps(&["b", "c"], &[])),
            ("c", deps(&["a"], &[])),
            ("b", deps(&["a"], &[])),
            ("a", deps(&[], &[])),
        ];
        let got = order_owned(&items).unwrap();
        assert_eq!(got[0], "a");
        assert_eq!(got[3], "d");
        let pos = |k: &str| got.iter().position(|x| x == k).unwrap();
        assert!(pos("a") < pos("b") && pos("a") < pos("c"));
        assert!(pos("b") < pos("d") && pos("c") < pos("d"));
    }

    #[test]
    fn drivers_without_manifest_deps_are_unconstrained() {
        // (kind, None) — lazy manifest: no edges, name order.
        let got = order([("zzz", None), ("aaa", None)]).unwrap();
        assert_eq!(got, ["aaa", "zzz"]);
    }
}
