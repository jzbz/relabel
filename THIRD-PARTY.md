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
and Unicode-3.0 - plus Apache-2.0 for `flagset` and Zlib for `foldhash`, each
admitted by name, on the six desktop targets, leaving out test-only crates. Anything else fails the check,
and the failure names the crate and the path that brought it in, so admitting
a licence is an edit to that file, reviewed like any other. It is not run in
CI yet.

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

relabel does not ship a binary yet. With no second argument the notices cover
the whole workspace, which is a superset. The binary a release ships is the
GUI's, so pass `crates/relabel-gui/Cargo.toml` as the second argument, and the
notices cover what that binary links and nothing else. The GUI's graph holds
btleplug's tree since it gained the printer link (see The printer link, below),
so its notice file needs the network as the workspace's does, and it has to be
generated again before any release.

The script appends the embedded fonts' licence, `crates/relabel-render/fonts/
OFL.txt`, after the crates' notices; cargo-about knows only crates. See Fonts,
below.

What the notice file does not cover:

- **The Rust standard library**, linked into every binary under MIT OR
  Apache-2.0. It is not a crate in the graph, so it is not listed.
- **Apache-2.0 `NOTICE` files.** cargo-about does not read them. No crate in
  the graph ships one; a crate that does needs its `NOTICE` added by hand.

It also lists more than it must: compile-time-only crates - proc-macros and
their dependencies - appear although they are not linked. That is harmless.

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
| ICU4X - `icu_*`, `zerovec`, `yoke`, `tinystr` and their kin - via `parley` | Unicode-3.0 | The Unicode licence's notice, with the binary or in its documentation. Line breaking and bidi run on ICU's data, so this is linked, not only compiled against. |
| `flagset` 0.4, via `datamatrix` | Apache-2.0 only | Its licence text, under section 4(a). There is no `NOTICE` file, and no copyright line to reproduce: the licence appendix is the unfilled template, and the only attribution is its author, Nathaniel McCallum. The first Apache-2.0-only dependency, admitted by name in `deny.toml`: it has no MIT or BSD branch, and Apache-2.0 is not on the allowlist. |

Seven crates ship no licence file in their packages. For `bluez-async` and
`bluez-generated`, on Linux, the text is fetched from upstream when the notices
are generated. The five macOS `objc2` crates publish none anywhere, so theirs is
the standard MIT text, with no copyright line to reproduce.

`siphasher`, via usvg, ships only a `COPYING` note naming its copyright
holders - "The Rust Project Developers" and Frank Denis - and its two licences.
Neither licence text exists in its package or in its repository at the
published commit, so that note, pinned by checksum in `about.toml`, is what the
notice file reproduces for it.

No crate in the graph ships an Apache-2.0 `NOTICE` file, so section 4(d) of
that licence asks nothing today.

Three crates offer Zlib alone, and none asks anything of a binary: Zlib
requires its notice only in a source distribution.

- **`foldhash`** is compiled in, as `hashbrown`'s default hasher, which parley
  and fontique ask for. `deny.toml` admits it by name.
- **`slotmap`** is compiled in with the GUI, under Slint's `fontdb` and
  `taffy`, and is admitted by name with the rest of that stack (see below).
- **`zlib-rs`** is an optional dependency of `flate2` that no feature enables.
  It is locked, never compiled.

## Artwork decoders

`relabel-render` imports raster and SVG artwork, so it ships `image` 0.25.10
with its PNG, JPEG, BMP, GIF and WebP codecs - `png`, `zune-jpeg`, `gif`,
`image-webp` and their kin - `base64` for the PNG a label embeds, and `resvg`
and `usvg` 0.48 with `roxmltree`, `svgtypes`, `simplecss` and `kurbo`. Every
one is MIT or offers MIT, so none needs more than the notice, and `cargo deny`
passes with `deny.toml` unchanged.

What is left out matters as much: AVIF, whose decoder links a C library;
resvg's `text`, which would put `fontdb` and the Zlib-only `slotmap` into
`relabel-render`'s own graph, not only the GUI's; and its `raster-images`. Two
side effects are worth knowing. resvg asks for tiny-skia's default features, so
tiny-skia's PNG codec is compiled in - the same `png` crate `image` brings. And
`pico-args`, a command-line parser that resvg and usvg depend on for their own
binaries, is compiled but never called.

## File dialogs, and the file format's key order

`relabel-gui` opens and saves labels through the platform's own file dialog,
with `rfd` 0.17.2 on its default features. What that adds differs by platform:
`rfd` and `pollster` 0.4 on Linux, `rfd` alone on Windows, and `rfd` and
`dispatch2` 0.3 on macOS. `rfd` is MIT, `pollster` offers MIT in the older
`Apache-2.0/MIT` spelling, which cargo-about reads as a choice, and `dispatch2`
offers MIT beside Zlib and Apache-2.0. `rfd` carries two MIT notices, its own
("Copyright (c) 2022 Bartłomiej Maryńczak") and one for the portal
window-identifier code it took from ashpd ("Copyright (c) 2020 Bilal
Elmoussaoui"), which sits deeper than cargo-about looks, so `about.toml` names
both files. `dispatch2` publishes no licence text anywhere, so it joins the
crates `packaging/third-party-notices.sh` accepts the canonical text for.
rfd's `gtk3` feature stays off: it would bring nine C `-sys` crates and
`target-lexicon`, whose licence `deny.toml` refuses.

`relabel-render` turns on `serde_json`'s `preserve_order`, so the keys a later
version wrote keep the order the file had them. That links `indexmap` 2.14
(Apache-2.0 OR MIT), which the build already compiled for a proc-macro but no
desktop binary linked; its `hashbrown` already ships. It also turns on
`float_roundtrip`, which links nothing. `cargo deny` passes with `deny.toml`
unchanged.

## The printer link

`relabel-gui` depends on `relabel-core` for its printer link, which puts
`btleplug` 0.13 and its tree into the GUI binary. None of it is new to the
workspace: `relabel-cli` already had every crate. What each target gains is
read from `cargo tree` against the GUI before the link, and the GUI's notice
file, which covers every desktop target, gains 19 crates. On every desktop
target: `btleplug`, whose licence is in the table
above; `tokio`, `tokio-macros`, `tokio-stream`, `tokio-util`, `dashmap` and
`bytes`, MIT; and `lock_api` and `parking_lot_core`, MIT OR Apache-2.0. On
Linux only: `mio`, MIT, and `socket2`, MIT OR Apache-2.0, which tokio's IO
driver brings there through `dbus-tokio`; `bluez-async` and
`bluez-generated`, MIT OR Apache-2.0, whose text the script fetches; `dbus`,
`dbus-tokio` and `libdbus-sys`, offering MIT in the older `Apache-2.0/MIT`
spelling; and `serde-xml-rs` and `xml`, MIT. On macOS only:
`objc2-core-bluetooth`, which offers MIT beside Zlib and Apache-2.0 and is one
of the five `objc2` crates given the standard MIT text. Windows and macOS also
gain `async-trait`, the `futures` crates and, on macOS, `static_assertions`,
all MIT OR Apache-2.0 and all already in the notices through the Linux graph,
so they add nothing to the file. `cargo tree --target all` also shows `wasi`,
for a target relabel does not build, which the notices leave out. `cargo
deny` passes with `deny.toml` unchanged, and `about.toml` already clarified
`btleplug`, `bluez-async` and `bluez-generated`, so it is unchanged as well.
The GUI's notice file therefore now carries btleplug's BSD-3-Clause notices.
On Linux the GUI now links `libdbus-1` as well (see System libraries).

## Arriving with work in progress

- **Slint, for the GUI.** Arrived with `relabel-gui`. Offered as
  `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR
  LicenseRef-Slint-Software-3.0`; relabel takes the royalty-free licence, which
  is written for desktop applications and is the reason the GPL branch - which
  would put the whole binary under GPL-3.0 - is never the one taken. It forbids
  use in an embedded system and exposing Slint's API, neither of which this
  does.

  **Section 2 is an obligation this project has not yet met.** It requires
  either Slint's `AboutSlint` widget in an About screen reachable from the
  application's top level, or Slint's attribution badge on a public page,
  preferably where the binaries are downloaded. The licence names the widget
  itself, so an About screen with only a line of text naming Slint is not what
  it asks for. The first slice of the editor has no About screen at all, so
  **no binary may be released until one of the two is in place.**

  It is also the one licence in the notice file that cargo-about cannot carry:
  a `LicenseRef` has no canonical text and a clarification does not give the
  tool one. `packaging/licenses/LicenseRef-Slint-Royalty-free-2.0.md` holds the
  text, checked on every run against the copy each Slint crate ships, and
  `packaging/third-party-notices.sh` appends it the way it already appends the
  fonts' OFL.

  The build brings **24 crates `deny.toml` names one by one**: the ten Slint
  crates, eight Apache-2.0-only ones (`winit`, `accesskit_winit`, `ab_glyph`,
  `ab_glyph_rasterizer`, `owned_ttf_parser`, `unicode-linebreak`,
  `linked_hash_set`, `gethostname`), `dpi` whose `Apache-2.0 AND MIT` is a
  conjunction rather than a choice, `imgref` on CC0-1.0, `libloading` on ISC,
  `slotmap` on Zlib, and `clipboard-win` and `error-code` on BSL-1.0. That is
  fewer than the thirty this section used to estimate, because the GUI takes
  the software renderer rather than femtovg and so brings no OpenGL stack.

  Twenty-five of its crates publish no licence text of any kind, so the notice
  reproduces the canonical text for them; they are named in
  `packaging/third-party-notices.sh`. Eleven more publish one without saying
  where, and `about.toml` now points at each, so their copyright lines survive.

## Fonts

`relabel-render` embeds three faces, unmodified, from the Noto project's
`latin-greek-cyrillic` family. They are under the SIL Open Font License 1.1,
not relabel's MIT licence.

| File | Version | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `NotoSans-Condensed.ttf` | 2.015 | 617,784 | `806816f70ef268abf388acdacf6b2e169f58f70c9f063515eef16a1f256eace6` |
| `NotoSans-CondensedSemiBold.ttf` | 2.015 | 620,208 | `55ebf2188d1b9ff6b09affe3783c7704f8bd627586bf0a097ea4f6a8842f8cce` |
| `NotoSansMono-Condensed.ttf` | 2.014 | 596,712 | `d60154ae4ecc9f2e8a1333de367b5decff30715f50dd1eca0048cd68fd905fd6` |

- **Source.** The hinted static builds published at
  `notofonts/notofonts.github.io`, commit
  `92d2ea744479d3379927f1fb5f8b0945dd0349b7`, under
  `fonts/NotoSans/hinted/ttf/` and `fonts/NotoSansMono/hinted/ttf/`. The same
  versions are released as `NotoSans-v2.015` and `NotoSansMono-v2.014` in
  `notofonts/latin-greek-cyrillic`, and the same bytes ship in Fedora 44's
  `google-noto-sans-fonts` and `google-noto-sans-mono-fonts` 20251201-2.
- **Licence.** `crates/relabel-render/fonts/OFL.txt` is the one in
  `notofonts/latin-greek-cyrillic` at the `NotoSans-v2.015` tag, commit
  `c4a321e123e4d4ff315f57f4e0adf294fe3a95be`, and carries the fonts' copyright
  line: "Copyright 2022 The Noto Project Authors". It declares no Reserved
  Font Name, and the files are not modified in any case.
- **What the licence asks.** The fonts may be bundled and redistributed with
  the program, provided every copy carries the copyright notice and the
  licence (section 2); the notice file does. They may not be sold by
  themselves (section 1). Labels printed with them are documents, which the
  licence does not reach (section 5). The name table's "Noto is a trademark
  of Google LLC." is a trademark notice, separate from the licence.
- **What they cover.** Latin, Greek and Cyrillic, with the currency signs, the
  degree, micro and ohm signs, and general punctuation. Not CJK, Arabic,
  Hebrew or Thai, and the proportional faces have no arrows. A character none
  of the three has is refused by name rather than drawn as an empty box.

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
system's `libdbus-1` at build time and loads it at run time: in `relabel-cli`
and, since the printer link, in the GUI, where before the GUI only opened it
with `dlopen` through rfd. Building either therefore needs the libdbus-1
development files, found through `pkg-config`. It is not built into relabel -
the crate's `vendored` feature is off - and nothing ships it.
Should a bundle ever carry it, an AppImage for instance, libdbus's own licence
applies and belongs here; it has not been reviewed.

On Linux, rfd's file-chooser backend loads `libdbus-1.so.3` with `dlopen` at run
time - the library the GUI already links for btleplug - and runs the `zenity`
program when the desktop has no file-chooser portal. Neither is built into
relabel or shipped with it.

## Code copied into this repository

None. If that changes, the policy is `docs/DESIGN.md` section 9's: every copied
file keeps its copyright line and gets an entry here.

## Data in this repository

**`vendor/devices.json`** is a normalised snapshot of NIIMBOT's own device
registry, taken from the NIIMBOT Android app, version 6.6.7
(`assets/DevicesModule_en.json`), on 2026-09-05, as the file itself records.
`relabel-proto`'s build script compiles it into the model table, so it is in
every binary. No licence accompanies it.

It is interoperability facts - model ids, resolutions, print widths, density
ranges - which `docs/DESIGN.md` section 9 places in the tier anyone may use. The
EU's sui generis database right does not apply to it. That right protects only
databases whose makers are established in the EU, and this one's maker is
NIIMBOT, a Chinese company; and it rewards investment in gathering data that
already exists, where this file records NIIMBOT's own products.
