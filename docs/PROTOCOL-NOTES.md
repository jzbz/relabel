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

## 3. Observed on real hardware

First contact, 2026-09-05: a **D110_M**, wire id 2320, firmware 4.17, hardware
4.01, over BLE on Linux/BlueZ. Negotiated MTU 244.

**A D110_M is not a D110.** Different wire id, a different position on the
ladder, and a different print flow. The name is misleading and the registry is
the thing to trust.

**The negotiated level is the flow, and it is easy to be off by one.** This unit
answers `Connect` with result 3 - "level 2 or better" - and then resolves to
**level 4** from the status reply, whose two version bytes read 3 and 1 for
`3 * 100 + 1 = 301`. Level 4 wants the **nine-byte `PrintStart` and thirteen-byte
`SetPageSize`**, no `PageStart`, the label type set before the density, and two
deliberately discarded packets. An earlier reading of this table mapped level 4
to the seven-byte flow; the printer would have acknowledged that and printed
nothing useful.

**The heartbeat charge byte is the 0-4 enum, not a percentage.** Sources
disagreed on this. The `PrinterInfo` battery sub-type and the heartbeat charge
byte were read seconds apart and both returned `3`, which settles it: they are
the same coarse scale, and 3 means about three quarters.

**The heartbeat temperature byte is plausibly Fahrenheit.** It read `0x49` = 73
on a printer that had been idle at room temperature. As Celsius that is
implausible; as Fahrenheit it is 22.8 C, which is exactly a room. One reading is
not proof, but it is the first evidence either way and it explains why the value
has always looked too high.

**The status reply carries more than a version.** This unit reported a single
packet ceiling of 9000, a maximum send size of 200, a packet interval of 0, and
no colour support. relabel surfaces these in `info` and does **not** act on them:
their exact semantics are unconfirmed, and clamping writes to a misread field
would break models this has never run against. Worth revisiting if a wide-head
printer ever rejects a row that fits the length byte.

**Parsers confirmed against real replies.** Model id from a two-byte payload,
serial as ASCII from a ten-byte one, both version fields as `major.minor`, and
the battery enum all decoded correctly first time.

**The tag layout is confirmed too**, against a genuine roll: eight raw UUID
bytes, a length-prefixed barcode, a length-prefixed serial, then the total and
used counts as unsigned 16-bit, then the consumable type - and **no trailing
capacity field**, which exercises the branch that treats it as optional. A
hand-decode of the captured bytes matched the parser field for field. The
counts must be read unsigned: this roll reported 192 total, but the widely
copied reference reads them signed, which would turn a roll above 32767 into a
negative number.

## 3a. Observed during a real print

The first successful print, same unit: a 96 x 200 dot label, 122 row packets,
about 3 KB on the wire, complete in roughly 5 seconds.

**The two progress percentages run in sequence, not in parallel.** The print
figure ramps 0 to 100 while the rows are consumed, and only then does the feed
figure ramp 0 to 100 while the paper moves; the page counter increments last.
So neither percentage on its own means the job is over, and a client that
stopped at `print == 100` would cut the label off mid-feed. Judging completion
on the page counter is not conservatism, it is the only correct reading.

**The undocumented `u16` at `data[4..6]` of a print status is not a counter.**
It is worth recording how this was got wrong, because the first reading was
convincing. On the first print it stood at 8877, rose to 8998, and went flat - a
rise of 121 while 122 row packets were sent, which looked exactly like a
per-packet counter and would have made a real dropped-row check possible.

A second print settled it, and settled it the other way. That job sent 194 row
packets, so a counter would have resumed above 8998 and finished near 9192.
Instead the value was **below** 8998 for most of the job and **plateaued on 8998
again**. It is not cumulative, and the 121-versus-122 match was coincidence.

What it does do is ramp while printing and level off at the same figure every
time, which is the signature of a physical quantity reaching a steady state
rather than of anything being counted. Print-head temperature is the obvious
candidate - 8998 would be 89.98 C in hundredths, which is a believable working
temperature for a thermal head - but that is a guess from two samples and the
units are unconfirmed. relabel surfaces the raw bytes and acts on none of it.

**Single-dot features print.** The ruler on the second test label draws its
ticks one dot wide, and the thinnest of its four calibration bars is a single
dot; both came out cleanly. So no columns are being lost across the head, the
bit packing is right way round, and the practical resolution really is the full
203 dpi rather than something coarser after the round trip.

**No rows are dropped or duplicated.** A dotted diagonal running the length of
the label came out evenly spaced end to end across 194 row packets. A dropped
row would have shown as a kink and a duplicated one as a stutter.

**The label on this roll is 40 mm x 12 mm**, or 320 x 96 dots, which fills the
printable area almost exactly: the solid block placed hard against the far end
of the image printed complete, with no bare paper past it. That was measured by
printing a ruler on the label itself - a tick every millimetre, a taller one
every five - which is a reasonable trick for any roll whose size is unknown,
because **the printer never reports label dimensions**. The tag carries a
barcode and a used/total count, not millimetres, and the barcode-to-size lookup
is a cloud endpoint this project does not call.

**`GetPaperInfo` does not answer the label-size question.** This was the most
promising lead for it, since the command is documented for protocol version 4
and above and this printer reports exactly 4. It does reply - `0x59` returns
`0x69` - but with only **four bytes**, `00 18 00 18`: two 16-bit values, both 24
dots, or 3 mm at 203 dpi. That is a believable gap height and nothing else. The
documented long form needs nine bytes or more before paper width, direction and
tail length appear, and this firmware sends four, so none of that arrives.
Neither figure is anywhere near the 40 mm the label actually measures.

So the size problem stands: a preset table plus a way for the user to measure
is the answer, not a protocol query. Printing a millimetre ruler on the label
is the cheapest measuring tool and it needs no new protocol work.

**The sacrificial sends went out and nothing objected.** The discarded status
request after `PrintStart` and the discarded heartbeat after `PrintEnd` are both
visible in the log, and the job completed cleanly with them in place. That does
not prove they are *needed* on this model - only that they are harmless.

## 3b. Observed printing a run of more than one label

A three-label run on the same unit printed the first label and then stopped:
the second job's page counter never left 0, the 25 s deadline expired, and
nothing came out of the printer.

**The job-level commands of a second job are refused, for a moment.** Job 2's
`SetLabelType`, `SetDensity` and `PrintStart` each answered a bare `0x00` -
`5555 33 01 00 32`, `5555 31 01 00 30`, `5555 02 01 00 03` - which are byte for
byte the vendor SDK's refuse constants `v`, `p` and `N0`, against their accept
twins `u`, `o` and `M0` ending `01 33`, `01 31` and `01 02`. The page-level
commands of the same job were accepted in the same breath, so the firmware was
discriminating rather than deaf. relabel checked that byte only on
`PrintClear`, `PageStart` and `PageEnd`, and the V5 flow sends neither of the
first two - so it streamed a whole page into a printer with no job open, and
then waited out its deadline for a counter that could never move.

**It means "not yet", not "no".** Sending the refused command again, a fifth of
a second later, prints the label. The vendor does exactly this:
`DataSend.printRepeatRequestAndTimeoutWithRefuseProcessing` resends up to
`sRepeatRequestCounts`, which is 10. relabel now resends the settings and
`PrintStart` on a bare `0x00`, and fails with the step named if the refusals
never stop. What the printer is busy with is presumably the previous label
still feeding out, but that is an inference: the only fact is the byte.

**`data[7]` of a print status appears to track whether a job is open.** It read
`01` through every poll of a real print and `00` through all 157 polls of the
stalled job. This firmware sends an eight-byte status, so the byte is inside
what arrives; relabel does not read it, and one trace is not enough to act on
it.

**A third reading of `data[4..6]`, agreeing with 3a.** It ramped 8878 to 8998
while label 1 printed, then sat flat on 8877 for the 23.5 s in which the
printer did nothing at all. An idle figure below the working one is the
signature of something settling, not of something being counted - though the
vendor's own logging names a field in this region `sparePacket`, so the two
readings are still unreconciled. relabel acts on neither.

## 4. Still open after this pass

The SDK shows what the vendor *sends*. It cannot show what the firmware
*enforces*, so these remain hardware questions:

- Whether the three counter bytes in a `0x85` row are validated by firmware, or
  merely computed by convention.
- What `Cancel` and `Pause` do to paper position mid-page.
- The printhead-width disagreements between third-party tables (D11_H 142 vs
  178, K4 832 vs 656). That is vendor catalogue data, not protocol.
- The units and polarity of the heartbeat temperature and RFID flags.
- How a user learns their label size without the vendor's cloud SKU lookup.
