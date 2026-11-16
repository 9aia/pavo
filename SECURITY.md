# Security Policy

Report vulnerabilities privately by emailing
luisfuturist+9aia@gmail.com with "SECURITY" in the subject — do
not open public issues, do not use GitHub's advisory or
private-reporting features, and do not disclose details publicly;
contact us first and we will coordinate disclosure privately.

## Scope

- `pavo serve` binds `127.0.0.1` only — the bridge (`/api/*`) and the
  static web host are localhost surfaces, not a network service.
- Drivers are discovered executables (`tamias-driver-<type>` on
  `PATH` or in `$TAMIAS_HOME/drivers/`) — installing a driver means
  trusting that code; pavo spawns them and speaks ndjson JSON-RPC.
- Driver credentials resolve via `cuora://` refs — pavo itself holds
  no secret values.
- The wire contract is versioned and additive-only within v1; unknown
  fields/kinds degrade rather than fail.

## Notes

We respond to reports as quickly as we can; expect acknowledgement
within a few days.
