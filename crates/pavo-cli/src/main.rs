#![cfg_attr(coverage, feature(coverage_attribute))]
//! pavo — the CLI surface of the viz host. Scaffold stub.
//!
//! Planned verbs (P-003): `drivers` (list discovered view drivers),
//! `view <driver>:<view> [--format json|dot|mermaid]`, `open
//! <driver>[:<view>]` (→ TUI), `dashboard`. `--json` everywhere;
//! exit codes CI-meaningful. The CLI doubles as the protocol
//! conformance harness — every view is streamable.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "pavo",
    version,
    about = "Visualization host for the 9aia tool suite"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List discovered view drivers (tamias-driver-* on PATH / drivers dir).
    Drivers,
    /// Render a driver view — `pavo view myco:board --format json`.
    View {
        /// `<driver>:<view>` deep link.
        target: String,
        /// Output format.
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Open a driver view in the TUI.
    Open {
        /// `<driver>` or `<driver>:<view>`.
        target: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Drivers) => {
            eprintln!(
                "pavo: scaffold — driver discovery not implemented yet (see TODO in pavo-core)"
            );
            std::process::exit(2);
        }
        Some(Command::View { .. }) | Some(Command::Open { .. }) => {
            eprintln!("pavo: scaffold — view rendering not implemented yet");
            std::process::exit(2);
        }
    }
}
