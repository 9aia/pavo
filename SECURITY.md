# Security Policy

Report vulnerabilities privately to the maintainers via GitHub:
https://github.com/9aia/pavo/security/advisories/new

Do not open public issues for security reports.

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
