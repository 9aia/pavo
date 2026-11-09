//! The ndjson serve loop — one [`Request`] per line on stdin, one
//! [`Response`] per line on stdout. Nothing else may touch stdout.
//!
//! This is the loop every pavo driver re-implements identically
//! (`myco-driver`, `cuora-driver`, `sepia-driver-pavo`,
//! `tamias-driver-tamias`, `sturn-driver`, `mimus-driver`,
//! `bupha-driver`, `pavo-driver-config`); the kit absorbs it.

use std::io::{self, BufRead, Write};

use pavo_protocol::wire::{parse_request, response_line, Request, Response};

/// Run the serve loop until `input` reaches EOF or the client goes
/// away.
///
/// Each non-blank line is parsed as a wire [`Request`]; parse failures
/// (malformed JSON, unknown `op`) become `ok:false` responses — a
/// driver must never drop a line silently or crash the wire. Blank
/// lines are skipped. `dispatch` maps a parsed request to its
/// response; keep it cheap and side-effect-light for `manifest`.
///
/// Termination: stdin EOF ends the loop normally. A write or flush
/// failure means the host went away — the loop stops and reports
/// `Ok(())`, matching every existing driver's "exit quietly" rule.
/// Stdin read errors likewise end the loop.
///
/// # Errors
/// Currently infallible in practice — the signature returns
/// [`io::Result`] so callers can `?` it inside larger `main`s.
pub fn serve<R, W, F>(input: R, output: W, mut dispatch: F) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&Request) -> Response,
{
    let mut out = output;
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match parse_request(line) {
            Ok(req) => dispatch(&req),
            // Malformed JSON or an unknown op — the conformance
            // contract wants `ok:false`, never a dropped line.
            Err(e) => Response::err(e.to_string()),
        };
        // A response that can't serialize still owes the client a
        // line — emit the fallback error shape by hand.
        let text = response_line(&response).unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"response serialization failed\"}".to_string()
        });
        if writeln!(out, "{text}").is_err() || out.flush().is_err() {
            return Ok(()); // client went away
        }
    }
    Ok(())
}

/// [`serve`] over the process's real stdin/stdout — the `serve` verb
/// body for a `tamias-driver-<type>` binary.
///
/// ```no_run
/// # use pavo_driver_kit as kit;
/// # use pavo_protocol::wire::{Request, Response};
/// fn dispatch(req: &Request) -> Response {
///     Response::err("not implemented")
/// }
/// fn main() {
///     let _ = kit::serve_stdio(dispatch);
/// }
/// ```
///
/// # Errors
/// See [`serve`] — propagates only unexpected I/O failures.
pub fn serve_stdio(dispatch: impl FnMut(&Request) -> Response) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(stdin.lock(), stdout.lock(), dispatch)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::wire::WireError;
    use serde_json::{json, Value};

    fn lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(out)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// Requests round-trip; blank lines skip; bad ops and bad JSON
    /// both answer `ok:false`.
    #[test]
    fn serve_dispatches_and_reports_errors() {
        let input = concat!(
            "{\"op\":\"manifest\"}\n",
            "\n",
            "   \n",
            "{\"op\":\"jobs\"}\n",
            "{\"op\":\"bogus\"}\n",
            "not json\n",
        );
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, |req| match req {
            Request::Manifest => Response::ok(json!({"name": "t"})),
            Request::Jobs => Response::ok_typed(&Vec::<u8>::new()),
            _ => Response::err("unexpected"),
        })
        .unwrap();
        let got = lines(&out);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0]["ok"], json!(true));
        assert_eq!(got[0]["result"]["name"], json!("t"));
        assert_eq!(got[1]["ok"], json!(true)); // jobs → []
        assert_eq!(got[1]["result"], json!([]));
        assert_eq!(got[2]["ok"], json!(false)); // unknown op
        assert!(got[2]["error"].as_str().unwrap().contains("bogus"));
        assert_eq!(got[3]["ok"], json!(false)); // invalid JSON
    }

    /// A dead client (write failure) ends the loop quietly — Ok, not
    /// an error, matching every existing driver's exit rule.
    #[test]
    fn serve_tolerates_dead_client() {
        struct Dead;
        impl Write for Dead {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let input = "{\"op\":\"manifest\"}\n{\"op\":\"manifest\"}\n";
        serve(input.as_bytes(), Dead, |_| Response::ok(json!({}))).unwrap();
    }

    /// `serve` is generic over any BufRead/Write pair — &[u8] and
    /// Vec<u8> suffice for tests and embedders.
    #[test]
    fn parse_error_is_wire_error_not_panic() {
        assert!(matches!(
            parse_request("{\"op\":\"nope\"}"),
            Err(WireError::UnknownOp { .. })
        ));
    }
}
