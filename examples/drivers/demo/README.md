# tamias-driver-demo

The reference pavo view driver — a self-contained Python 3 script
(stdlib only) that exercises the ViewNode IR: table + kv + markdown on
the board, a `graph` view, a `tree` view, and contributions to the
navbar, dashboard, and statusbar slots.

## Install

The executable must be named `tamias-driver-<type>` and discoverable
either on `PATH` or in `$TAMIAS_HOME/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`):

```bash
# on PATH
export PATH="$PWD/examples/drivers/demo:$PATH"

# or into a tamias drivers dir
mkdir -p "${TAMIAS_HOME:-~/.local/share/tamias}/drivers"
ln -s "$PWD/examples/drivers/demo/tamias-driver-demo" \
      "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/tamias-driver-demo"
```

Requires `python3` (3.8+; nothing beyond the stdlib).

## Views

| Deep link         | Route          | Shows                                          |
| ----------------- | -------------- | ---------------------------------------------- |
| `demo:board`      | `/demo/board`  | counts (kv) + items table + markdown footer    |
| `demo:deps`       | `/demo/deps`   | crate dependency `graph` — try `--format mermaid` |
| `demo:timeline`   | `/demo/timeline` | milestone `tree`, expandable                 |

`demo:board` accepts `--param status=open|wip|done|blocked` to filter
the items table.

## Queries

`items`, `counts`, `deps`, `events` — all canned-but-plausible JSON.

```bash
pavo query demo items --param status=open
pavo view demo:deps --format dot
```

## Chrome

Contributes `navbar` (board + deps entries), one `dashboard` panel
(`/demo/board`), and a `statusbar` entry.
