# tamias-driver-platform

The platform-features demo driver — a self-contained Python 3 script
(stdlib only) that exercises the chrome slots and platform feeds:

| Feature | Where |
| --- | --- |
| `contributes.sidebar` | two left-sidebar entries (`/platform/board`, `/platform/jobs`) |
| `contributes.headerActions` | top-bar actions opening the jobs/board views |
| `contributes.notifications` + `jobs` | provider markers; item data via the `notifications`/`jobs` wire ops |
| `deps` | `optional: ["config"]` — ordering/degradation is the host's job |
| `QueryBinding.driver` | the board's `config-theme` binding is resolved cross-driver by the host against `tamias-driver-config` |

## Install

Same discovery rules as every `tamias-driver-*`: on `PATH` or in
`$TAMIAS_HOME/drivers/`:

```bash
export PATH="$PWD/examples/drivers/platform-demo:$PATH"
# or: ln -s "$PWD/examples/drivers/platform-demo/tamias-driver-platform" \
#       "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/"
```

Requires `python3` (3.8+; nothing beyond the stdlib) and — for the
cross-driver binding to resolve — `tamias-driver-config` discoverable
(`cargo xtask install` provides it). Without `config`, the
`config-theme` binding degrades to an `{"error": …}` payload.

## Views

| Deep link           | Route              | Shows                                             |
| ------------------- | ------------------ | ------------------------------------------------- |
| `platform:board`    | `/platform/board`  | snapshot kv + config kv (foreign query) + items table |
| `platform:jobs`     | `/platform/jobs`   | the job feed as a table                           |

## Platform feeds

```bash
pavo chrome                  # sidebar/headerActions/notifications/jobs slots
pavo jobs                    # aggregated feed — platform's two canned jobs
pavo notifications           # aggregated feed — one read, one unread alert
pavo view platform:board     # config-theme resolves via tamias-driver-config
```
