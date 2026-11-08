//! Tamias home resolution — copied from `tamias-driver` (the helpers
//! are tiny and pavo must not depend on tamias crates; the *layout*
//! is the shared contract, not the code).
//!
//! `<tamias-home>` resolves as `$TAMIAS_HOME` →
//! `$XDG_DATA_HOME/tamias` → `~/.local/share/tamias` → `./.tamias`.
//! Pavo-capable drivers live alongside install drivers in
//! `<tamias-home>/drivers/` — one dir, `manifest`/`capabilities`
//! disambiguates (P-003 locked decision: no second discovery dir).

use std::path::PathBuf;

/// `<tamias-home>` — `$TAMIAS_HOME` → `$XDG_DATA_HOME/tamias` →
/// `~/.local/share/tamias` → `./.tamias`.
#[must_use]
pub fn tamias_home() -> PathBuf {
    std::env::var_os("TAMIAS_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("tamias")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/tamias")))
        .unwrap_or_else(|| PathBuf::from("./.tamias"))
}

/// The `<tamias-home>/drivers` dir — where installed products drop
/// their `tamias-driver-*` executables.
#[must_use]
pub fn drivers_dir() -> PathBuf {
    tamias_home().join("drivers")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    // One test, sequential env mutation — parallel tests mutating the
    // same vars would race each other (and anything reading env).
    #[test]
    fn tamias_home_resolution_precedence() {
        let tmp = tempfile::tempdir().unwrap();

        // $TAMIAS_HOME wins outright.
        let custom = tmp.path().join("custom-home");
        std::env::set_var("TAMIAS_HOME", &custom);
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("xdg"));
        assert_eq!(tamias_home(), custom);
        assert_eq!(drivers_dir(), custom.join("drivers"));

        // Without it, $XDG_DATA_HOME/tamias.
        std::env::remove_var("TAMIAS_HOME");
        assert_eq!(tamias_home(), tmp.path().join("xdg/tamias"));

        std::env::remove_var("XDG_DATA_HOME");
    }
}
