//! Wire conformance — the built `tamias-driver-config` binary must
//! pass the full pavo driver contract (`pavo-testkit::check_driver`).

#![cfg_attr(coverage, feature(coverage_attribute))]
#![allow(clippy::unwrap_used)]

use std::path::Path;

#[test]
fn driver_passes_pavo_conformance() {
    let violations =
        pavo_testkit::check_driver(Path::new(env!("CARGO_BIN_EXE_tamias-driver-config")));
    assert!(
        violations.is_empty(),
        "conformance violations:\n{}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
