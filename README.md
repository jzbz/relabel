# relabel

A desktop app for NIIMBOT thermal label printers. Finds the printer over
Bluetooth, shows you what it is and what is loaded in it, lets you compose a
label, and prints it. Linux, macOS and Windows, one codebase.

Rust throughout, Slint for the GUI. No webview, no Qt, no C++, no vendor SDK.

**Status: early.** The protocol layer, the BLE transport, the session and the
job engine are written and tested against an in-memory printer, and
`relabel-cli` has printed on real hardware: a D110_M, over Bluetooth, on
40 × 12 mm labels. That is one printer. Every other model in the registry is
untested, so treat any claim about one as unverified. `relabel-render` has the
label document format and draws text, rectangles, lines, linear barcodes, QR
codes, DataMatrix symbols and placed images, with halftoning for photographs.
It binds a label to rows of CSV data and checks every row before anything
prints, and `relabel-cli label` prints the result. The GUI makes, opens, edits
and saves labels, connects to a printer over Bluetooth, shows exactly what will
print on it, and prints it, up to 99 copies at a time; printing from the GUI
has yet to be checked on paper. See
[Milestones](docs/DESIGN.md#7-milestones) and
[the renderer's plan](docs/RENDER-PLAN.md) for the order things land in.

Not affiliated with, endorsed by, or connected to NIIMBOT or Gengcon. "NIIMBOT"
is their trademark and is used here only to say which printers this talks to.

## Why

The existing options each miss something. `niimblue` has the best protocol
coverage anywhere but is a Chromium-only web app. `Thermalith` is native and
cross-platform but GPL-3.0 and has no Bluetooth at all. `NiimPrintX` has BLE and
five models, and is GPL-3.0 and Python/Tk. Nothing is native, cross-platform,
Bluetooth-first and MIT at once.

## Layout

| Crate | Contents |
| --- | --- |
| `crates/relabel-proto` | Frame codec, command set, payload builders and parsers, image encoder, print-flow selection, model registry. Bytes in, bytes out — no I/O, no async, no GUI types, one dependency. |
| `crates/relabel-core` | Transports (BLE, and an in-memory printer), the session that owns reassembly and request/response matching, the print-job engine, the printer link the GUI talks to on a thread of its own, and the CSV reader for variable data. |
| `crates/relabel-render` | The label document format, and the rasteriser that turns it into dots: whole-dot geometry, halftoning with per-element zones, text, images and SVG, linear barcodes, QR and DataMatrix, and a calibration ruler. Variable data, the preflight that checks every row before a run, fold-over and cable-flag geometry, and the pixels a preview pane draws. No device, no clock, no async, no GUI types. |
| `crates/relabel-cli` | Headless driver for hardware bring-up. Not a shipped binary. |
| `crates/relabel-gui` | The desktop app, in Slint. Starts a new label or opens one, draws it at whole dots, edits it by pointer or keyboard, and saves it through the platform's own file dialog. Connects to a printer, shows the exact dots a job will send it, and prints them, one job per label. Early. |

`relabel-gui` starts, opens, edits and saves labels, connects to a printer,
shows what will print, and prints it.

The split is deliberate. `relabel-proto` is pure and synchronous, so the fuzz
target builds in a second and the protocol tests need neither a Bluetooth stack
nor a font. Anything that touches a device belongs a layer up.

## Testing without a printer

`relabel-core` ships an in-memory printer that decodes what the session writes
using the same accumulator the real path uses, and replies as a device would —
including delivering several frames in one read, or splitting one frame across
several, which is what a real BLE link does and what most clients get wrong.

The whole session and job engine is tested against it on a simulated clock, so
`cargo test` needs no hardware, no Bluetooth stack and no permissions:

```bash
cargo test --workspace
```

That covers the command sequence and payload widths each print flow emits, that
`PrintEnd` still goes out on the error path, that unsolicited packets never
satisfy a pending request, and that a frame too large for the link is refused
rather than split. What it cannot cover is what firmware actually *enforces* —
see `docs/PROTOCOL-NOTES.md`.

## What is known, and how well

`docs/DESIGN.md` is the specification the implementation is written against. It
is unusually explicit about provenance because the protocol is not documented by
its vendor: every claim that sources disagree on is marked, and the
[disputed-claims ledger](docs/DESIGN.md#appendix--disputed-claims-ledger) at the
end records what relabel does about each one and why.

`docs/DESIGN-GAPS.md` is the adversarial read of that document — what is
under-researched, what contradicts itself, and what still needs a printer to
settle.

`docs/PROTOCOL-NOTES.md` records what a first-party reading of the vendor's
publicly distributed Android SDK confirmed and corrected. It exists so the Rust
is written against a prose note rather than against a decompiler window, and it
is explicit that no vendor code, output or binary is reproduced anywhere in this
repository. Two of its corrections are load-bearing: `cols` is the image width
clamped to the print head (which reconciles two clients that appeared to
contradict each other), and **the D11/D110/D101 family never uses the seven-byte
`PrintStart`** that the most widely copied open-source table assigns to D101.

Two things are worth knowing up front:

- **The model registry is the vendor's own.** `vendor/devices.json` is a dated,
  normalised snapshot of NIIMBOT's device database, recovered from their Android
  app: 79 printers with wire model ids, resolution, printhead widths, density
  ranges, print direction, RFID type and supported label types, plus the label
  sensing and consumable enumerations resolved to English. `build.rs` generates
  the Rust table from it. Nothing is fetched at build time — a build that
  reaches the network is not reproducible and cannot run in a Flatpak sandbox.
- **`printhead_px` is derived, not measured.** It is
  `ceil(print_width_mm × dots_per_mm)`, which agrees with every published figure
  that can be checked independently — but two third-party tables disagree with
  each other on about a dozen models, and only a full-width test print settles
  those.

## Reverse engineering, and what this project will not do

Protocol behaviour here comes from open-source reimplementations and published
packet captures. Interoperability information is not copyrightable, and reverse
engineering for interoperability is protected in the US (17 USC §1201(f)) and
the EU (Software Directive Art. 6). No code is transliterated from a GPL
implementation or from the vendor's SDK, and the SDK binary is neither used nor
redistributed.

Four deliberate limits:

1. **RFID is read-only, always.** Reading a roll's tag for label type and counts
   is ordinary interoperability. Writing tags, cloning them, or resetting the
   print counter is not something this project does.
2. **No firmware flashing.** Implementing a flasher against a reverse-engineered
   protocol is the one feature that turns a bug report into a dead printer.
3. **No region-lock circumvention.** The region check is a server-side call the
   vendor's app makes; a third-party client is unaffected by simply not making
   it. That is a fact, not a feature.
4. **The trademark stays nominative.** "works with NIIMBOT printers", never in
   the name, the icon or a domain.

## Build and run

Needs a recent stable Rust. On Linux, BLE goes through BlueZ, so `bluetoothd`
must be running and the D-Bus system bus reachable, and building either
`relabel-cli` or the GUI needs the libdbus-1 development files, which
`libdbus-sys` finds through `pkg-config`.

Inside a container (distrobox, toolbox, or plain podman) the host's system bus
is usually mounted but not where the D-Bus client looks for it. Point it at the
host socket:

```bash
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/host/run/dbus/system_bus_socket
```

```bash
cargo test --workspace
```

## Bring-up

`relabel-cli` is how the stack gets pointed at a printer while something is
being brought up. The GUI's own route is Control+P, which opens the print pane
on the open label; Connect in it finds the printer and identifies it, and Print,
with Copies beside it, prints what the pane shows:

```bash
cargo run -p relabel-gui -- crates/relabel-render/tests/corpus/v1/kitchen-sink.relabel
```

Encode an image without touching any hardware, and see what would go on the
wire:

```bash
cargo run -p relabel-cli -- print --dry-run --preview --head 96 label.png
```

Find a printer:

```bash
cargo run -p relabel-cli -- scan
```

Connect and report what it says about itself — model, wire id, negotiated
protocol level, print head, density range, firmware, battery, and which reads it
declined to answer:

```bash
cargo run -p relabel-cli -- info
```

Print something:

```bash
cargo run -p relabel-cli -- print --copies 1 label.png
```

Print a `.relabel` file once for each row of the CSV file it names. Every row
is bound, drawn and checked first, and a row that would print a wrong or
missing mark stops the run before anything connects; `--check` stops there
either way, and `--dry-run` also encodes every label. A label that prints the
date needs the time zone spelled out, since nothing here can look one up:

```bash
cargo run -p relabel-cli -- label --check --now "$(date -Iseconds)" crates/relabel-render/tests/corpus/v1/variables.relabel
```

That one is refused on purpose: its third row's SKU is too long for the
barcode, and the output names the row, its line in the CSV and the element.

Useful flags while something is going wrong:

| Flag | What it is for |
| --- | --- |
| `--trace` | Dump every frame in both directions afterwards. The best thing to attach to a bug report. |
| `--pace-ms 0` | Remove the inter-write delay, to find out whether pacing is needed on this model at all. |
| `--zero-counters` | Send the three row counter bytes as zero, to find out whether the firmware checks them. |
| `--device <text>` | Pick a printer by name or id when there is more than one. |
| `raw 40 08` | Send a frame by hand — this one asks for the model id. |

The last two flags in that table exist because those questions are genuinely
open. See the bring-up checklist in
[docs/DESIGN.md](docs/DESIGN.md#81-things-a-hardware-session-settles).

## Licence

MIT. See [LICENSE](LICENSE). The licences of the crates relabel is built from,
and how the notice file that ships with each release is generated, are in
[THIRD-PARTY.md](THIRD-PARTY.md).
