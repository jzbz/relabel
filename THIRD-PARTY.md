# Third-party licences

relabel is MIT. This file records what it is built from, what shipping a
binary owes the people who wrote that code, and what is known to be coming. It
is a ledger, not the notice file itself: the full notice text that ships with a
release is generated, because a hand-kept copy of the notices for over a
hundred crates goes stale the first time a dependency updates.

## The notice file, and the gate in front of it

Two tools, two jobs.

**`cargo deny check licenses` is the gate.** `deny.toml` allows MIT and the
three licences today's graph cannot do without - BSD-2-Clause, BSD-3-Clause
and Unicode-3.0 - on the six desktop targets, leaving out test-only crates.
Anything else fails the check, and the failure names the crate and the path
that brought it in, so admitting a licence is an edit to that file, reviewed
like any other. It is not run in CI yet.

**`packaging/third-party-notices.sh OUTPUT` writes the notice file.** It runs
cargo-about 0.9.2 with `about.toml` and the plain-text template `about.hbs`:

```bash
cargo install --locked cargo-about@0.9.2 --features cli
```

For each crate it reproduces the licence text the crate ships, copyright lines
included, preferring MIT or BSD where a crate offers a choice, because those
texts carry the copyright line and Apache-2.0's does not. It needs the network:
`bluez-async` and `bluez-generated` ship no licence file in their packages, so
their MIT text - "Copyright (c) 2020 Luis Félix" - is fetched from the upstream
commit each was published from.

cargo-about's own `--fail` lets several incomplete results through with a zero
exit: a pinned clarification that no longer matches, a fetch that failed, and a
crate's own licence text silently replaced by the generic one, with no
copyright line. The script fails on all three. One fallback is accepted: the
five macOS `objc2` crates - `block2`, `objc2`, `objc2-encode`,
`objc2-foundation` and `objc2-core-bluetooth` - publish no licence text
anywhere, only a note saying they are MIT, so they get the standard MIT text
and their author's name.

Every clarification in `about.toml` is pinned by SHA-256. When a dependency
update changes one of those files, the script fails, and the checksum is
re-derived after reading what changed.

relabel does not ship a binary yet, so for now the notices cover the whole
workspace, which is a superset. Once the GUI crate exists, pass its
`Cargo.toml` as the second argument, and the notices cover what that binary
links and nothing else.

What the notice file does not cover:

- **The Rust standard library**, linked into every binary under MIT OR
  Apache-2.0. It is not a crate in the graph, so it is not listed.
- **Apache-2.0 `NOTICE` files.** cargo-about does not read them. No crate in
  the graph ships one; a crate that does needs its `NOTICE` added by hand.

It also lists more than it must: compile-time-only crates - proc-macros and
their dependencies - appear although they are not linked. That is harmless, and
the only reason Unicode-3.0 is in the file.

## Licences that need more than an MIT notice

In today's graph, on the desktop targets. relabel does not ship a binary yet,
so this is the workspace's graph; every other crate in it is MIT, or offers MIT
as one branch of an OR.

| Crate | Licence | What a binary owes |
| --- | --- | --- |
| `tiny-skia`, `tiny-skia-path` 0.12 | BSD-3-Clause | The notice and disclaimer, reproduced: "Copyright (c) 2011 Google Inc." and "Copyright (c) 2020 Yevhenii Reizner". Neither name may be used to endorse relabel. |
| `arrayref` 0.3 | BSD-2-Clause | The notice and disclaimer, reproduced: "Copyright (c) 2015 David Roundy". |
| `btleplug` 0.13 | BSD-3-Clause, with MIT and Apache-2.0 parts | Its whole `LICENSE.md`. The metadata says `MIT/Apache-2.0/BSD-3-Clause`, which reads like a choice and is not one: btleplug itself is BSD-3-Clause ("Copyright (c) 2020-2021, Nonpolynomial"), and it carries code from Rumble under MIT or Apache-2.0 and from Blurmac under BSD-3-Clause. |
| `moxcms` 0.8, `pxfm` 0.1 | BSD-3-Clause OR Apache-2.0 | BSD-3-Clause is the branch taken: "Copyright (c) Radzivon Bartoshyk". |

Seven crates ship no licence file in their packages. For `bluez-async` and
`bluez-generated`, on Linux, the text is fetched from upstream when the notices
are generated. The five macOS `objc2` crates publish none anywhere, so theirs is
the standard MIT text, with no copyright line to reproduce.

No crate in the graph ships an Apache-2.0 `NOTICE` file, so section 4(d) of
that licence asks nothing today.

Two licences appear in `Cargo.lock` and still ask nothing of a binary:

- **`zlib-rs` (Zlib)** is an optional dependency of `flate2` that no feature
  enables. It is locked, never compiled. Zlib asks for no notice in a binary in
  any case.
- **`unicode-ident` (Unicode-3.0)** is reached only through `proc-macro2` and
  `syn`, which run at compile time and are not linked.

## Arriving with work in progress

- **`flagset` 0.4.7 - Apache-2.0 only**, via `datamatrix`, for DataMatrix
  symbols. The first dependency with no MIT branch. Apache-2.0 section 4(a)
  requires its text to ship with the binary. It has no `NOTICE` file, and no
  copyright line: its licence appendix is the unfilled template, and the only
  attribution is its author, Nathaniel McCallum. Its arrival needs a
  crate-scoped exception in `deny.toml`.
- **Slint, for the GUI.** Offered as `GPL-3.0-only OR
  LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`. relabel
  takes the royalty-free licence, whose section 2 requires either Slint's
  `AboutSlint` widget in an About screen reachable from the application's top
  level, or Slint's attribution badge on a public page, preferably where the
  binaries are downloaded. The licence names the widget itself, so an About
  screen with only a line of text naming Slint is not what it asks for. The
  same licence forbids use in an embedded system and exposing Slint's API. The
  GPL branch is never the one taken; it would put the whole binary under
  GPL-3.0.

  A Slint build brings about thirty more crates that `deny.toml` rejects
  today - Apache-2.0-only ones, ISC, Zlib, BSL-1.0 and CC0-1.0, measured on a
  scratch copy of this workspace with the Slint features rpgp uses - so the
  allowlist grows then, deliberately.
- **Bundled fonts**, under the SIL Open Font License 1.1, get an entry here
  each, with whether the licence names a Reserved Font Name - a subset is a
  derivative, and a Reserved Font Name would force a rename.

## Watch list: weak copyleft

Neither is in the graph. Both are named in the design documents, so both are
recorded before they arrive rather than discovered after.

- **`serialport` 4 (MPL-2.0)**, for the USB and serial transport.
- **`option-ext` 0.2 (MPL-2.0)**, which `dirs` brings in through `dirs-sys`,
  planned for the GUI's config and recent-files paths.

MPL-2.0 is file-level copyleft. relabel's own code stays MIT and the program as
a whole can ship under MIT, but for every MPL-covered crate in a binary the
release must include the MPL-2.0 text and tell recipients where that crate's
source can be obtained (section 3.2(a)); a modified MPL file would have to be
published under MPL. `serialport`'s package carries only the two-line MPL
notice, not the licence text, so the text has to come from elsewhere.
`deny.toml` rejects MPL-2.0, so admitting either crate is a deliberate change to
that file.

## System libraries

On Linux, `btleplug` reaches BlueZ through `libdbus-sys`, which links the
system's `libdbus-1` at build time and loads it at run time. It is not built
into relabel - the crate's `vendored` feature is off - and nothing ships it.
Should a bundle ever carry it, an AppImage for instance, libdbus's own licence
applies and belongs here; it has not been reviewed.

## Code copied into this repository

None. If that changes, the policy is `docs/DESIGN.md` section 9's: every copied
file keeps its copyright line and gets an entry here.

## Data in this repository

**`vendor/devices.json`** is a normalised snapshot of NIIMBOT's own device
registry, taken from the NIIMBOT Android app, version 6.6.7
(`assets/DevicesModule_en.json`), on 2026-09-05, as the file itself records.
`relabel-proto`'s build script compiles it into the model table, so it is in
every binary. No licence accompanies it.

It is treated as interoperability facts - model ids, resolutions, print widths,
density ranges - which `docs/DESIGN.md` section 9 places in the tier anyone may
use. Whether a collection of such facts attracts the EU's sui generis database
right is a separate question, and it is open, not settled.
