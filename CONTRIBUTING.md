# Contributing

Run `pnpm check` before opening a pull request. Automated tests must not touch installed games.

A change to the format has to do one of two things: read every repack already out there, or
raise `FORMAT_VERSION` in `crates/kryo/src/format.rs`. Update `docs/format.md` with it.

Anything that can change a byte on the way through needs a test in
`crates/kryo/tests/roundtrip.rs`. A repack that installs a broken game is the worst thing this
tool can do, and the check at the end of a freeze is there to make sure it never ships.

`src/ui/` holds copies of Kryoto Desktop's controls, icons and lettering. Change them in Desktop
first. `crates/setup/src/mark.rs` and `src/ui/ascii/mark.ts` are traced from kryo.to's K//: never
redraw it.

`crates/setup` must keep starting on a clean Windows install: no runtime, no WebView, nothing
from outside the `.exe`.

When preparing a release, keep the versions in `package.json`, `Cargo.toml` and
`src-tauri/tauri.conf.json` the same.
