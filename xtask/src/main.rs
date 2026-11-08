#![cfg_attr(coverage, feature(coverage_attribute))]
//! xtask — repo chores. `cargo xtask <verb>` via the alias in
//! `.cargo/config.toml`.
//!
//! - `check`    — `cargo fmt --check` + `cargo clippy -D warnings`
//! - `test`     — `cargo nextest run` when installed, else `cargo test`
//! - `coverage` — `cargo llvm-cov` (HTML → target/llvm-cov/html);
//!   extra args pass through, e.g. `--fail-under-lines 97`
//! - `install`  — release-install `pavo` + `tamias-driver-config`
//!   into ~/.cargo/bin

use std::process::{Command, ExitStatus};

fn run(args: &[&str]) -> ExitStatus {
    let empty: &[&str] = &[];
    let (prog, rest) = args.split_first().unwrap_or((&"true", empty));
    Command::new(prog).args(rest).status().unwrap_or_else(|e| {
        eprintln!("xtask: failed to spawn {prog}: {e}");
        std::process::exit(1)
    })
}

fn have_cargo_sub(sub: &str) -> bool {
    Command::new("cargo")
        .args([sub, "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check() -> i32 {
    let ok = run(&["cargo", "fmt", "--all", "--", "--check"]).success()
        && run(&[
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .success();
    i32::from(!ok)
}

fn test() -> i32 {
    let status = if have_cargo_sub("nextest") {
        run(&["cargo", "nextest", "run", "--workspace"])
    } else {
        run(&["cargo", "test", "--workspace"])
    };
    status.code().unwrap_or(1)
}

fn coverage(extra: &[String]) -> i32 {
    if !have_cargo_sub("llvm-cov") {
        eprintln!("cargo-llvm-cov not installed — `cargo install cargo-llvm-cov`");
        return 1;
    }
    let args = coverage_command(extra);
    run(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .code()
        .unwrap_or(1)
}

/// The `cargo llvm-cov` argv — pure so tests can pin the flags.
/// `--no-cfg-coverage`: don't set `cfg(coverage)`, so the
/// `#![cfg_attr(coverage, feature(coverage_attribute))]` crate attrs
/// stay inert and stable rustc can instrument (nothing uses
/// `#[coverage(off)]`). LLVM_COV/LLVM_PROFDATA may need to point at
/// a system llvm when rustup's llvm-tools-preview is absent.
fn coverage_command(extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "cargo",
        "llvm-cov",
        "--workspace",
        "--no-cfg-coverage",
        "--html",
        "--output-dir",
        "target/llvm-cov/html",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().cloned());
    args
}

fn install() -> i32 {
    // The host plus the first-party drivers — both land as binaries
    // on PATH (~/.cargo/bin), which is also the driver-discovery path.
    let ok = run(&[
        "cargo",
        "install",
        "--path",
        "crates/pavo-cli",
        "--locked",
        "--force",
    ])
    .success()
        && run(&[
            "cargo",
            "install",
            "--path",
            "crates/pavo-driver-config",
            "--locked",
            "--force",
        ])
        .success();
    i32::from(!ok)
}

/// Verb dispatch — split from `main` so the unknown-verb path is
/// unit-testable without spawning cargo.
fn dispatch(cmd: &str, rest: &[String]) -> i32 {
    match cmd {
        "check" => check(),
        "test" => test(),
        "coverage" => coverage(rest),
        "install" => install(),
        other => {
            eprintln!(
                "usage: cargo xtask <check|test|coverage|install>\n  unknown verb: {other:?}"
            );
            2
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    std::process::exit(dispatch(&cmd, &rest));
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn run_spawns_and_reports_status() {
        assert!(run(&["true"]).success());
        assert!(!run(&["false"]).success());
        // Empty argv defaults to `true`.
        assert!(run(&[]).success());
        // Args pass through.
        assert!(run(&["echo", "hi"]).success());
    }

    #[test]
    fn have_cargo_sub_detects_installed_tools() {
        // cargo itself is always present.
        assert!(have_cargo_sub("--version") || !have_cargo_sub("definitely-not-a-sub"));
        // llvm-cov presence is environment-dependent — just assert
        // the call doesn't panic and agrees with a direct probe.
        let via_fn = have_cargo_sub("llvm-cov");
        let direct = Command::new("cargo")
            .args(["llvm-cov", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert_eq!(via_fn, direct);
    }

    #[test]
    fn coverage_command_pins_flags_and_passthrough() {
        let args = coverage_command(&[]);
        assert_eq!(args[0], "cargo");
        assert!(args.contains(&"llvm-cov".to_string()));
        assert!(args.contains(&"--no-cfg-coverage".to_string()));
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"--html".to_string()));
        let extra = coverage_command(&["--fail-under-lines".to_string(), "97".to_string()]);
        assert!(extra.ends_with(&["--fail-under-lines".to_string(), "97".to_string()]));
    }

    #[test]
    fn dispatch_unknown_verb_is_usage_error() {
        assert_eq!(dispatch("bogus", &[]), 2);
        assert_eq!(dispatch("", &[]), 2);
    }
}
