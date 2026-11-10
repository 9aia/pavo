//! Ops manifest → a generated clap CLI (feature `cli`).
//!
//! [`command`] builds the parser: one subcommand per op (params →
//! args per [`ParamKind`]) plus one per entry in `surfaces` (trailing
//! argv passthrough — unbounded surfaces stream). [`run`] parses an
//! argv, builds the wire params map, dispatches through a [`Surface`],
//! prints `stdout`/`stderr`, and returns the engine's exit code.
//!
//! Every subcommand also accepts `--root <dir>` — the repo/dir hint
//! wire requests already carry (`params.root`); drivers that resolve
//! roots from params honor it, others ignore it.

use std::io::Write;

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value};

use super::{OpOutcome, OpsManifest, ParamKind, Surface};

/// clap's `Str`/`Id` want `'static` — op/param names are runtime
/// data, and a generated CLI parses once per process, so leaking
/// is the honest cost.
fn s(v: String) -> &'static str {
    Box::leak(v.into_boxed_str())
}

/// Build the generated CLI's clap parser for `manifest`. `bin` is the
/// displayed program name (`kuda`, `tamias-driver-cli`, …).
#[must_use]
pub fn command(bin: &str, manifest: &OpsManifest) -> Command {
    let mut cmd = Command::new(s(bin.to_string()))
        .about(format!(
            "{} ops — generated CLI over its tamias driver",
            manifest.engine
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("root")
                .long("root")
                .value_name("DIR")
                .global(true)
                .help("repo/dir override — passed through as the `root` wire param"),
        );
    for op in &manifest.ops {
        let mut sub = Command::new(s(op.name.clone())).about(s(op.doc.clone()));
        if op.mutates {
            sub = sub.after_help("mutates engine state");
        }
        let mut positional = 0usize;
        for param in &op.params {
            let arg = match param.kind {
                ParamKind::Arg => {
                    positional += 1;
                    Arg::new(s(param.name.clone()))
                        .required(true)
                        .index(positional)
                }
                // `--name`, `--name v`, `--name=v` — bare flag spells
                // `true`, a present value rides as a string.
                ParamKind::Opt => Arg::new(s(param.name.clone()))
                    .long(s(param.name.clone()))
                    .value_name("VALUE")
                    .num_args(0..=1)
                    .default_missing_value("true"),
                ParamKind::Flag => Arg::new(s(param.name.clone()))
                    .long(s(param.name.clone()))
                    .action(ArgAction::SetTrue),
            };
            let arg = match &param.doc {
                Some(doc) => arg.help(s(doc.clone())),
                None => arg,
            };
            sub = sub.arg(arg);
        }
        cmd = cmd.subcommand(sub);
    }
    for surface in &manifest.surfaces {
        cmd = cmd.subcommand(
            Command::new(s(surface.clone()))
                .about(format!(
                    "{surface} — engine surface (argv passthrough, streams)"
                ))
                .arg(
                    Arg::new("args")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                ),
        );
    }
    cmd
}

/// Parse `argv` (argv[0] included) against [`command`], dispatch the
/// matched op/surface through `surface`, print captured output, and
/// return the exit code. clap errors/help print and exit `2`/`0`
/// through clap's own rendering.
#[must_use]
pub fn run(bin: &str, manifest: &OpsManifest, argv: &[String], surface: &mut dyn Surface) -> i32 {
    let matches = match command(bin, manifest).try_get_matches_from(argv) {
        Ok(m) => m,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let Some((name, sub)) = matches.subcommand() else {
        return 2; // arg_required_else_help makes this unreachable
    };
    if manifest.surfaces.iter().any(|s| s == name) {
        let args: Vec<String> = sub
            .get_many::<String>("args")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        return surface.surface(name, &args);
    }
    let Some(op) = manifest.op(name) else {
        eprintln!("unknown op: {name}");
        return 2;
    };
    let params = params_of(sub, op, &matches);
    let outcome = surface.op(op, &params);
    print_outcome(&outcome)
}

/// Subcommand matches → the `cli.<op>` wire params map.
fn params_of(sub: &ArgMatches, op: &super::OpSpec, top: &ArgMatches) -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(root) = top
        .get_one::<String>("root")
        .or_else(|| sub.get_one("root"))
    {
        params.insert("root".to_string(), Value::from(root.clone()));
    }
    for param in &op.params {
        match param.kind {
            ParamKind::Arg | ParamKind::Opt => {
                if let Some(v) = sub.get_one::<String>(&param.name) {
                    // bare `--name` lands as "true" — bools read as
                    // bools so `bool_param` consumers still work.
                    let value = match v.as_str() {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        _ => Value::from(v.clone()),
                    };
                    params.insert(param.name.clone(), value);
                }
            }
            ParamKind::Flag => {
                if sub.get_flag(&param.name) {
                    params.insert(param.name.clone(), Value::Bool(true));
                }
            }
        }
    }
    params
}

/// Outcome → stdout/stderr + the engine's own exit code.
fn print_outcome(outcome: &OpOutcome) -> i32 {
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let _ = stdout.lock().write_all(outcome.stdout.as_bytes());
    let _ = stdout.lock().flush();
    let _ = stderr.lock().write_all(outcome.stderr.as_bytes());
    let _ = stderr.lock().flush();
    outcome.code
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::surface::{OpSpec, ParamSpec};
    use serde_json::json;

    fn manifest() -> OpsManifest {
        OpsManifest::from_value(&json!({
            "engine": "fx",
            "ops": [
                {"query": "cli.find", "doc": "sym -> loc",
                 "params": [{"name": "symbol", "kind": "arg"},
                            {"name": "workspace", "kind": "flag"},
                            {"name": "config", "kind": "opt"}]},
                {"query": "cli.status", "doc": "drift"},
            ],
            "surfaces": ["watch"],
        }))
        .unwrap()
    }

    struct Spy {
        last: Option<(String, Map<String, Value>)>,
        surfaced: Option<(String, Vec<String>)>,
    }

    impl Surface for Spy {
        fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome {
            self.last = Some((op.name.clone(), params.clone()));
            OpOutcome {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }
        }
        fn surface(&mut self, name: &str, args: &[String]) -> i32 {
            self.surfaced = Some((name.to_string(), args.to_vec()));
            0
        }
    }

    #[test]
    fn ops_parse_into_wire_params() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "main", "--workspace", "--root", "/tmp/r"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        let (name, params) = spy.last.unwrap();
        assert_eq!(name, "find");
        assert_eq!(params["symbol"], json!("main"));
        assert_eq!(params["workspace"], json!(true));
        assert_eq!(params["root"], json!("/tmp/r"));
        assert!(params.get("config").is_none());
    }

    #[test]
    fn opt_accepts_value_and_bare_true() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "x", "--config", "fx.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(spy.last.unwrap().1["config"], json!("fx.toml"));

        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "x", "--config"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(spy.last.unwrap().1["config"], json!(true));
    }

    #[test]
    fn surfaces_stream_trailing_argv() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        // hyphen-leading trailing args pass through verbatim
        // (`--` itself is clap's escape token, not data).
        let argv: Vec<String> = ["fx", "watch", "--fast"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(
            spy.surfaced.unwrap(),
            ("watch".to_string(), vec!["--fast".to_string()])
        );
    }

    #[test]
    fn unknown_subcommand_is_clap_error() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "bogus"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 2);
    }

    #[test]
    fn spec_types_stay_constructible() {
        let op = OpSpec {
            name: "x".into(),
            doc: String::new(),
            params: vec![ParamSpec {
                name: "p".into(),
                kind: ParamKind::Flag,
                doc: None,
            }],
            mutates: true,
        };
        assert!(serde_json::to_value(&op).unwrap()["mutates"]
            .as_bool()
            .unwrap());
    }
}
