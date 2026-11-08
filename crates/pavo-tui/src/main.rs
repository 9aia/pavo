//! pavo-tui — the ratatui host surface for pavo.
//!
//! `pavo-tui` discovers pavo-capable tamias drivers (`Host::discover`),
//! merges their chrome contributions into a tab bar + status bar, and
//! interprets each driver's ViewNode IR natively on a three-pane
//! canvas: driver rail │ view canvas │ detail pane.
//!
//! Non-interactive modes:
//!
//! - `pavo-tui --list` — print discovered drivers + views, exit.
//! - `pavo-tui --help` — usage.
//!
//! Deep links: `pavo-tui demo:board`, `--view demo:board`, or
//! `--driver demo` (first view) land directly on a view.

#![cfg_attr(coverage, feature(coverage_attribute))]

mod app;
mod model;
mod worker;

use std::io::{self, Write};
use std::process::ExitCode;

use app::App;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

const USAGE: &str = "\
pavo-tui — ratatui host for pavo view drivers

USAGE:
    pavo-tui [OPTIONS] [DRIVER:VIEW]

ARGS:
    <driver:view>    deep link — resolve this view on start
                     (bare <driver> resolves its first view)

OPTIONS:
    --driver <kind>  open the first view of <kind>
    --view <target>  open <driver>:<view> (same as positional arg)
    --list           list discovered drivers + views, then exit
    -h, --help       show this help

KEYS:
    j/k arrows navigate · Tab/←/→ switch panes · Enter opens
    r refresh · d dump view JSON · g dump graph DOT · ? help · q quit
";

/// Parsed command line.
struct Args {
    /// Deep-link target to resolve after boot.
    initial: Option<String>,
    /// `--list` — print drivers + views and exit.
    list: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut initial: Option<String> = None;
    let mut list = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--list" => list = true,
            "--driver" => {
                let kind = iter
                    .next()
                    .ok_or_else(|| "--driver requires a value".to_string())?;
                initial = Some(kind);
            }
            "--view" => {
                let target = iter
                    .next()
                    .ok_or_else(|| "--view requires a value".to_string())?;
                initial = Some(target);
            }
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}")),
            s => {
                if initial.is_some() {
                    return Err(format!("unexpected extra argument: {s}"));
                }
                initial = Some(s.to_string());
            }
        }
    }
    Ok(Args { initial, list })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("pavo-tui: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.list {
        return ExitCode::from(worker::list() as u8);
    }
    match run_tui(args.initial) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pavo-tui: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Enter the alternate screen, run the event loop, restore on exit.
fn run_tui(initial: Option<String>) -> io::Result<()> {
    let worker = worker::spawn();
    let mut app = App::new(worker, initial);

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = loop {
        terminal.draw(|f| app.draw(f))?;
        if !app.step() {
            break Ok(());
        }
    };

    // Restore the terminal no matter how we got here.
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    let mut out = io::stdout();
    let _ = out.flush();
    result
}
