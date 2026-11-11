# Install a driver

Pavo discovers view providers as executables named
`tamias-driver-<type>` — the same convention tamias uses for content
drivers. There is no registry lookup and no config file: if the
binary is findable and answers `manifest`, pavo uses it.

## Discovery locations

1. **`PATH`** — any directory on your shell's `PATH`.
2. **`<tamias-home>/drivers/`** — `$TAMIAS_HOME` if set, else
   `$XDG_DATA_HOME/tamias`, else `~/.local/share/tamias`, else
   `./.tamias`. Symlinks work.

```bash
# on PATH (development)
export PATH="$PWD/examples/drivers/demo:$PATH"

# or into the tamias drivers dir (installed)
mkdir -p "${TAMIAS_HOME:-~/.local/share/tamias}/drivers"
ln -s "$PWD/examples/drivers/demo/tamias-driver-demo" \
      "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/tamias-driver-demo"
```

## What counts as a pavo driver

A `tamias-driver-*` executable is a pavo driver when its `manifest`
answers a [`DriverManifest`](/reference/driver-manifest) with
`"serve"` in `verbs` (or `"pavo"` in `capabilities`). Drivers that
only do tamias install verbs are ignored by pavo.

```bash
printf '{"op":"manifest"}\n' | tamias-driver-demo serve
pavo drivers    # confirm discovery
```

## Installing real product drivers

Product drivers ship from their own repos — `tamias-driver-kuda`
(kuda), `tamias-driver-myco` (myco), `tamias-driver-cuora` (cuora),
`tamias-driver-sturn` (sturn), `tamias-driver-tamias` (tamias),
`tamias-driver-mimus` (mimus), `tamias-driver-sepia` (sepia),
`tamias-driver-bupha` (bupha). Install them like any tamias package
(they are `tamias-driver-*` content) or drop the binary on `PATH`:

```bash
tamias install git:9aia/myco//libraries/myco   # the pack ships the bundled driver
# or install just the driver binary:
cargo install --git https://github.com/9aia/myco tamias-driver-myco
```

Once installed they are discovered exactly like the examples —
`pavo drivers` lists them, `pavo views` lists their views.

Compiled drivers can also ride inside tamias packages: stage
prebuilt binaries under `drivers/bin/<rustc-target-triple>/`
(e.g. `drivers/bin/x86_64-unknown-linux-gnu/tamias-driver-foo`) —
`tamias install --allow-drivers` picks the host triple, verifies the
payload digest, and lands an executable in `<tamias-home>/drivers/`.
See tamias's `examples/README.md` for the publish recipe.

## Verifying

```bash
./scripts/smoke-examples.sh   # repo root — runs the example set end-to-end
pavo-testkit                  # conformance: every driver passes the same suite
```

`pavo-testkit`'s `check_driver` runs the wire contract over any
driver binary; `FixtureDriver` builds fake drivers for host tests.
See [Write a driver](/how-to/write-a-driver).
