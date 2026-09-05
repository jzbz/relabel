# Protocol notes from vendor-SDK analysis

These are **facts** — opcode values, payload widths, field orderings, dispatch
rules — read off Gengcon's publicly distributed Android SDK and written down in
prose. They are recorded here so the Rust is written against this note rather
than against a decompiler window.

Interoperability information is not copyrightable, and reverse engineering for
interoperability is protected in the US (17 USC §1201(f), *Sega v. Accolade*)
and the EU (Software Directive Art. 6). No vendor code, decompiler output,
comment text or binary is reproduced here or anywhere in this repository.

**Provenance.** `JCPrinterSDK 4.0.2-release.aar`, sha256
`24a0d0ff408ee155d4858d9810513af431af78527d3accb15b32e4b1767c80af`, obtained
from a third-party public mirror, not through any vendor agreement — nothing
here is covered by an accepted EULA or NDA. The archive is R8-minified: the
`bean/`, `data/`, `util/`, `printerInterface/` and `controller/` packages keep
their original names, while the print-task and factory classes are reduced to
short identifiers. Where a class is named below by such an identifier it is
written as `«factory u»` and so on, because the identifier is an artefact of one
build and will not survive a version bump.

The binaries are kept outside the working tree and are never staged, vendored,
submoduled or shipped.

Dated 2026-09-05. Corroborating reads: NIIMBOT Android app 6.6.7
(`com.gengcon.android.jccloudprinter`), and the iOS `JCAPI.h` header.

---

## 1. What this confirmed

Each of these was already implemented in `relabel-proto` from open-source
sources, and each is now independently confirmed against a first-party reading.
That matters more than it sounds: the reverse-engineering literature looks like
seven agreeing implementations but is really **two lineages** plus a wiki by one
of their authors. The SDK is a genuine third, independent reading.

- **Frame layout.** `55 55 | CMD | LEN | DATA | XOR | AA AA`, `LEN` counting
  `DATA` only, `XOR` over `CMD ^ LEN ^ DATA`.
- **`SetPageSize` (`0x13`).** Rows first, then columns, each split big-endian by
  integer division and remainder against 256. The four-byte form carries exactly
  those two fields.
- **`0x84` empty row.** `LEN = 3`, payload `[row_hi, row_lo, repeat]` — the row
  index big-endian, then the run length. (The vendor's helper takes the repeat
  count as its first parameter and the two row bytes after it, which is why the
  ordering is worth stating explicitly.)
- **Row opcode semantics.** `0x83` builds *coordinate* data — the x-positions of
  black pixels. `0x84` builds *blank line* data. `0x85` builds *data pattern*
  data — the packed row. This matches the names used in `image.rs`.
- **Inverted lid polarity.** The model set that reports the cover flag inverted
  is keyed on model id and includes 512, 2304 and 2560 — the whole D11 family.

## 2. What this corrected

### 2.1 `cols` is the image width **clamped to the print head**

The single most dangerous open question in `DESIGN-GAPS.md`. Two working clients
disagree: one sends the label width, the other the printhead width.

**Both are right for the cases they were tested on.** The vendor sends
`min(image_width_px, printhead_px)`, with the printhead figure a constant baked
into each model family's print task — observed as 192 for the D101-class tasks
and 384 for the B1/B21-class tasks, which match this repository's derived table
exactly. Columns beyond the head are simply never fired.

`relabel-proto` sends the image width and **refuses** a page wider than the head
rather than clamping. The vendor can clamp safely because its own UI has already
constrained the canvas; a library that silently truncates a label and reports
success is the failure this crate exists to avoid. `relabel-render` is
responsible for fitting the canvas to the head.

### 2.2 D101 never uses the seven-byte `PrintStart`

`niimbluelib` maps D101 onto its `B1PrintTask`, which sends the seven-byte
`PrintStart`. The SDK's dispatch says otherwise, and D101 is a target model, so
this is load-bearing.

The dispatch is a switch on the reported model id selecting a factory, and each
factory then selects a print task from the negotiated protocol level held in one
integer field:

| Model | Factory | level 0 | level 1 | level 2 |
| --- | --- | --- | --- | --- |
| D11 (512, 1536) | «u» | base | mid | high |
| D110 (2304) | «t» | base | mid | high |
| D101 (2560) | «s» | base | mid | high |
| B1 (4096) | «l» | — level < 3 → mid — | | level ≥ 3 → **V4** |

The task classes sit in a shallow hierarchy, and **`PrintStart` width is
inherited from whichever base a task extends**:

```
base                     1-byte PrintStart, 4-byte SetPageSize
├── (D11-family level 0)
├── mid ──┬── (D11-family level 1)        inherits the 1-byte PrintStart
│         └── high                        2-byte PrintStart, 6-byte SetPageSize
│              ├── (D11-family level 2)
│              └── (B1 below level 3)
└──────────────── V4                      7-byte PrintStart
                   └── (B1 at level 3+)
```

So for **D11, D110 and D101 the `PrintStart` payload is one byte at protocol
levels 0 and 1, and two bytes at level 2. It is never seven bytes, and never
nine.** A nine-byte form and a `SendPageSizeBean` form exist but are reached
only by other families.

Two consequences beyond D101: **B1 below protocol level 3 also uses the
two-byte form**, not the seven-byte one; and the seven-byte form is gated on the
negotiated level rather than on the model.

The branch *names* — which level is "new" and which is "V3" — are inference from
the pattern, not from any retained string. The *hierarchy* is read directly and
is what determines the bytes.

### 2.3 The compression codec is miniLZO

`DESIGN.md` records the codec behind `0x87` / `0xA7` as documented nowhere. It
is **miniLZO**: the SDK ships `libminilzo.so` for two ABIs inside the AAR, and
the NIIMBOT app ships the same library. Two independent confirmations.

relabel still never compresses — run-merging plus `0x84` collapses realistic
label content well enough, and the framing for those opcodes carries a wider
length field whose checksum width is unspecified. But the gap is now a decision
rather than an unknown.

## 3. Still open after this pass

The SDK shows what the vendor *sends*. It cannot show what the firmware
*enforces*, so these remain hardware questions:

- Whether the three counter bytes in a `0x85` row are validated by firmware, or
  merely computed by convention.
- What `Cancel` and `Pause` do to paper position mid-page.
- The printhead-width disagreements between third-party tables (D11_H 142 vs
  178, K4 832 vs 656). That is vendor catalogue data, not protocol.
- The units and polarity of the heartbeat temperature and RFID flags.
- How a user learns their label size without the vendor's cloud SKU lookup.
