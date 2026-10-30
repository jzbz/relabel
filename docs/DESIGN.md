# relabel — Design Document

**An MIT-licensed, cross-platform Rust + Slint GUI for NIIMBOT thermal label printers.**

Status: specification. Nothing is built yet. This document is what the implementation gets written against; where the research is genuinely unresolved it says so and names the defensive choice rather than pretending.

---

## 0. What relabel is, and the conventions it inherits

A native desktop window that finds a NIIMBOT printer over Bluetooth Low Energy or USB, shows what it is and what is loaded in it, lets you compose a label out of text, barcodes, QR codes and images, and prints it. Linux, macOS and Windows, one codebase. Rust throughout, Slint for the GUI, no webview, no Qt, no C++, no Python subprocess.

The niche is real and currently empty. `niimblue` (MIT, Svelte) has the best protocol coverage in the ecosystem but is a Chromium-only web app whose standalone builds are Android and Windows only. `Thermalith` (C#, Avalonia) is the only comparable native cross-platform app but is GPL-3.0 and has no Bluetooth support at all — its README is explicit that USB-serial comes first and BLE comes later. `NiimPrintX` (Python/Tk) has BLE and covers five models, and is GPL-3.0. Nobody ships native + cross-platform + Bluetooth-first + broad model coverage. That is exactly relabel.

Conventions are taken from [rPGP](https://github.com/jzbz/rpgp) — the author's
Slint + Rust OpenPGP manager, referred to throughout as *rpgp* — deliberately
and without variation:

- Cargo workspace, `resolver = "3"`, `edition = "2024"`, `license = "MIT"`, an explicit `rust-version` floor that reflects what cargo will actually enforce (Slint 1.17 declares 1.92 throughout its tree, so 1.92).
- Core crates carry no GUI types. No `slint::` type reaches the protocol layer and no protocol type with a Slint dependency reaches a callback signature.
- `[workspace.dependencies]` entries are commented with *why the feature set is what it is*, not with what the crate does.
- Pure Rust where it is achievable, and an explicit "what is **not** pure Rust" table in the README where it is not.
- Long operations run off the event loop and report back through it.
- Prose is technical and specific. No marketing.

---

## 1. Protocol reference

Everything below is byte-level and implementable. Claims that seven independent implementations agree on are stated flatly. Claims where sources disagree are marked **[disputed]** with the resolution and the reasoning.

### 1.1 The standard frame

```
  0     1     2     3     4 .. 3+LEN     4+LEN   5+LEN 6+LEN
[0x55][0x55][CMD ][LEN ][DATA … LEN B ][ XOR ][0xAA][0xAA]
```

- Head `55 55`, tail `AA AA`, both literal, both two bytes.
- `LEN` is a **u8** counting DATA only. Fixed overhead is 7 bytes; frame length is `LEN + 7`; minimum 7 (LEN = 0), maximum 262.
- `XOR = CMD ^ LEN ^ data[0] ^ … ^ data[LEN-1]`. It **includes** CMD and LEN and **excludes** head, tail, and the Connect prefix. Initial value 0.
- Multi-byte fields inside DATA are **big-endian u16** — row number, page count, page dimensions, RFID counters — with exactly one exception, the bitmap black-pixel total, which is little-endian across two of three bytes (§1.8).

Self-check every example you transcribe. Two hex examples on the community wiki do not verify: the `TubeTypeAndWidth` get example prints checksum `0x0f` where `0x0e` computes, and the `PrintBitmapRow` example's prose contradicts its own hex.

Worked examples that do verify:

```
RfidInfo request   55 55 1a 01 01 1a aa aa      0x1a^0x01^0x01 = 0x1a
Heartbeat Adv2     55 55 dc 01 04 d9 aa aa      0xdc^0x01^0x04 = 0xd9
Connect            03 55 55 c1 01 01 c1 aa aa   0xc1^0x01^0x01 = 0xc1
PrintEmptyRow      55 55 84 03 00 00 0a 8d aa aa
```

Any "simple" packet — `LEN=1`, `DATA=[0x01]` — has checksum equal to CMD, because `N ^ 1 ^ 1 == N`. A "simple response" is `LEN=1`, `DATA=[0x01]` for success and `[0x00]` for failure.

**There is no escaping and no byte stuffing.** Payload bytes are freely `0x55` and `0xAA` — a bitmap row of alternating pixels is literally `55 55 55 …` — and the checksum byte itself can be either. **Framing must therefore be length-driven and never scan-for-magic-driven.** A decoder that splits on `55 55` corrupts every RFID response, every serial number, and every dense bitmap row. This is the single most consequential fact in the document.

`LEN` is a u8, so 255 bytes is a hard ceiling. The JS reference implementations silently truncate an over-length payload (`arr[pos] = data.length` on a `Uint8Array` wraps mod 256). **relabel's encoder returns an error instead.** Nothing legitimate overflows: the widest printhead in the catalogue is ET10 at 1600 px, giving stride 200 plus a 6-byte row header = 206-byte payload, 213-byte frame.

### 1.2 The two framing exceptions

**Firmware frames**, selected purely by command byte:

```
[55 55][CMD][IDX_HI][IDX_LO][LEN][DATA…][CRC32 big-endian, 4 B][AA AA]
```

Overhead 12, minimum 12 bytes. CRC-32 (IEEE/zlib) over `[CMD, IDX_HI, IDX_LO, LEN, …DATA]`. Applies to outbound `0x91 0x92 0x9B 0x9C` and inbound `0x90 0x9A 0x9D 0x9E`. Chunk size 200 bytes.

relabel **does not implement firmware upgrade** — see §9 — but the *decoder must still know these opcodes*, because it needs the size field at offset 5 and a 4-byte checksum to compute the frame length, and it needs that to resynchronise correctly. It parses the frame boundary and then discards the frame.

**Compress frames.** `CompressCommand 0x87` is documented with a u16 length field and `CompressImage 0xA7` with a u32 length field. Neither is implemented in any open-source project, the compression codec itself is documented nowhere, and the checksum width for these frames is unspecified. relabel never emits them and treats them as unparseable on receipt — the decoder drops one byte and resynchronises. That is a deliberate, documented gap, not an oversight.

**There is no "protocol V2" framing.** ConnectResult values, `protocolVersion` 0–5 and the seven print-task variants change command *sequences* and payload *widths*. The frame never changes.

### 1.3 The Connect 0x03 prefix — **[disputed, resolved]**

The `Connect` command (`0xC1`) is written by niimbluelib and everything downstream of it with a bare `0x03` byte prepended outside the head: `03 55 55 c1 01 01 c1 aa aa`. The byte is not covered by the checksum and does not appear on the `0xC2` reply.

Two adversarial reviews pushed back on this being a protocol fact, and they are right. Four independent implementations (`kahwee/thermark`, `prestonbrown/helixscreen`, `grossrc/NiimbotB1_Simple_Print`, `batkt/posApp`) send the prefix-free 8-byte frame through a uniform encoder with no `0x03` path. Four more (`kjy00302/niimprint`, `AndBondStyle/niimprint`, `labbots/NiimPrintX`, `eigger/hass-niimbot`) never send Connect at all and print successfully to D11/B21/B1 over RFCOMM and BLE.

**Resolution.** Send the prefix. It is harmless, it matches the most-exercised BLE client, and it is the only route to protocol-version negotiation. Encode it as a per-command flag on the outbound packet, not as a special case in a shared codec, and expose it as a connection setting so a stall can be bisected. Two hard rules follow:

1. **The receive decoder must never special-case a leading `0x03`.** `PageStart` is itself command `0x03`; a decoder that skips a leading `0x03` will mis-parse.
2. **Connect is not mandatory for printing.** If `0xC2` does not arrive, log it, assume `protocolVersion = 0`, and continue rather than aborting the session.

Untested and worth knowing: every implementation sends the prefix in the same write as the frame. Nobody has tried splitting it. Do not split it.

### 1.4 The receive decoder

This is the highest-risk piece of code in the project and gets the most testing. Both fragmentation directions are real:

- **One read or BLE notification can contain several whole frames.** Documented example: `55554a01044faaaa5555f60101f6aaaa` is two packets.
- **One frame can span reads.** Documented example: `5555d9091f90044c000001000016aaaa` arriving as `5555d9091f90044c000001000016` then `aaaa`. Over BLE, a reply longer than ATT_MTU−3 fragments; on an un-negotiated 23-byte MTU that is anything over 20 bytes, which includes RFID info, the 20-byte heartbeat and `PrinterStatusData`.

```rust
// Pseudocode. Reset the accumulator on disconnect.
loop {
    let i = find_subsequence(&buf, &[0x55, 0x55]) else {
        buf.retain_tail(1);          // a lone 0x55 may be half a head
        return;
    };
    buf.drain(..i);
    if buf.len() < 4 { return; }                       // head + cmd + len
    let cmd = buf[2];
    let (size_pos, ck_len) = if is_firmware_cmd(cmd) { (5, 4) } else { (3, 1) };
    if buf.len() <= size_pos { return; }
    let total = size_pos + 1 + buf[size_pos] as usize + ck_len + 2;
    if buf.len() < total { return; }                   // incomplete: wait
    if buf[total - 2..total] != [0xAA, 0xAA] { buf.drain(..1); continue; }  // resync
    match verify_and_parse(&buf[..total]) {
        Ok(pkt) => { emit(pkt); buf.drain(..total); }
        Err(_)  => { buf.drain(..1); }                 // bad checksum: resync
    }
}
```

Three implementation traps found in the reference code, none of which relabel replicates:

- niimbluelib's `processRawPacket` **discards the entire accumulated buffer** when byte 0 is not `0x55`, and **withholds already-complete packets** whenever a partial frame trails them in the same buffer.
- Both Python clients' `_recv()` loops spin forever on a partial frame (`while len(buf) > 4` with no `break`).
- `NiimPrintX` assumes one notification equals one packet.

The decoder is fuzzed (`cargo-fuzz`, `arbitrary`) against arbitrary byte streams with the invariants: never panics, never allocates unboundedly, and every emitted frame round-trips through the encoder to the same bytes.

### 1.5 Request → response correlation

**The frame carries no sequence number or transaction id.** Responses correlate by command byte alone, which forces **at most one outstanding request at a time**. relabel serialises all request/response traffic behind an async mutex in the session layer. One-way packets (bitmap rows) bypass the mutex only in the sense that they never wait; they still go through the same write queue so ordering is preserved.

Four families:

- **`+0x01`** (17 commands): `0x01→0x02, 0x03→0x04, 0x05→0x06, 0x07→0x08, 0x09→0x0A, 0x0B→0x0C, 0x13→0x14, 0x15→0x16, 0x1A→0x1B, 0x1C→0x1D, 0x70→0x71, 0x8E→0x8F, 0xC1→0xC2, 0xC3→0xC4, 0xE3→0xE4, 0xF3→0xF4, 0xF5→0xF6`.
- **`+0x10`** (20 commands): `0x20→0x30, 0x21→0x31, 0x22→0x32, 0x23→0x33, 0x26→0x36, 0x27→0x37, 0x28→0x38, 0x2D→0x3D, 0x2E→0x3E, 0x2F→0x3F, 0x54→0x64, 0x58→0x68, 0x59→0x69, 0x5A→0x6A, 0x5C→0x6C, 0xA2→0xB2, 0xA3→0xB3, 0xA5→0xB5, 0xA6→0xB6, 0xAF→0xBF`.
- **Echo, same id** (7): `0x0D, 0x0E, 0x0F, 0x11, 0x12, 0x17, 0x19`.
- **Exceptions** (6): `0x40 PrinterInfo → 0x40 + subtype` (0x41…0x4F); `0xDC Heartbeat → 0xD9 | 0xDD | 0xDE | 0xDF`; `0xDA CancelPrint → 0xD0`; `0x86 PrinterCheckLine → 0xD3`; `0x89 GetCompress → 0x88`; `0x1E → 0x1E or 0x1F`.

Because response ids reuse request opcode values (`0x0D 0x0E 0x0F 0x11 0x12 0x17 0x19`), **`RequestId` and `ResponseId` are two distinct Rust enums**, never one shared type. niimbluelib's `RequestCommandId` also carries a sentinel `Invalid = -1` that does not fit a `u8`; relabel has no such variant — an unknown opcode is `ResponseId::Unknown(u8)`.

**Never answered** (8): `0x83, 0x84, 0x85, 0x8A` (bitmap rows) and `0x91, 0x92, 0x9B, 0x9C` (firmware). Plus `0x87` and `0xA7`, which relabel never sends.

**Universal error replies** that can arrive in place of *any* expected response and must short-circuit the wait: `0xDB In_PrintError` (`data[0]` = error code) and `0x00 In_NotSupported`.

**Unsolicited packets** that arrive at any time and must not be mistaken for a reply: `0xD3 In_PrinterCheckLine`, `0xE0 In_PrinterPageIndex`, `0xC6 In_ResetTimeout`, and heartbeat replies if a heartbeat timer is running. The session's response matcher keys on the expected response id and routes everything else to an event channel.

**Two minor disputes, both harmless.** `0x1E` — niimbluelib accepts `0x1E` or `0x1F`, the wiki lists only `0x1F`; accept both. `0x11`/`0x12` — niimbluelib maps each to itself, the wiki maps `0x11→{0x11,0x12}` and `0x12→0x11`; accept either for both. Neither command is on any print path.

### 1.6 Command table

Client → printer, with the response id:

| | | | |
|---|---|---|---|
| `0x01` PrintStart → `0x02` | `0x03` PageStart → `0x04` | `0x05` PrinterLog → `0x06` | `0x07` PrinterConfig2 → `0x08` |
| `0x09` GetKeyFunction → `0x0A` | `0x0B` AntiFake → `0x0C` | `0x0D` GetPrintQuality → `0x0D` | `0x0E` TubeSettings → `0x0E` |
| `0x0F` TubeTypeAndWidth → `0x0F` | `0x10` SetOutNet → `0x10` | `0x11` SetLocalTemplate → `0x11`/`0x12` | `0x12` GetTimeFormat → `0x11`/`0x12` |
| `0x13` SetPageSize → `0x14` | `0x14` PageHeight → `0x14` | `0x15` PrintQuantity → `0x16` | `0x17` TemplateHistory → `0x17`,`0x63` |
| `0x19` PrinterUsage → `0x19` | `0x1A` RfidInfo → `0x1B` | `0x1C` RfidInfo2 → `0x1D` | `0x1E` ReverseFeed/MarginTop → `0x1E`/`0x1F` |
| `0x1F` MarginTopHeight → `0x1F` | `0x20` PrintClear → `0x30` | `0x21` SetDensity → `0x31` | `0x22` SetPrintSpeed → `0x32` |
| `0x23` SetLabelType → `0x33` | `0x26` SetLanguageType → `0x36` | `0x27` SetAutoShutdown → `0x37` | `0x28` PrinterReset → `0x38` |
| `0x2C` SetPrintMode → `0x3C` | `0x2D` SetLabelMaterial → `0x3D` | `0x2E` SetVolumeLevel → `0x3E` | `0x2F` SetAntiSetter → `0x3F` |
| `0x40` PrinterInfo → `0x41`–`0x4F` | `0x52` PrintingHistory → `0x62`/`0x63` | `0x54` RfidSuccessTimes → `0x64` | `0x58` SoundSettings → `0x68` |
| `0x59` GetPaperInfo → `0x69` | `0x5A` PrintTestPage → `0x6A` | `0x5C` HalfCut → `0x6C` | `0x70` GetVolume/WriteRFID → `0x71` |
| `0x83` BitmapRowIndexed → — | `0x84` EmptyRow → — | `0x85` BitmapRow → — | `0x86` PrinterCheckLine → `0xD3` |
| `0x87` CompressCommand → — | `0x89` GetCompress → `0x88` | `0x8A` BitmapRowDoubleColor → — | `0x8E` PositioningCalibration → `0x8F` |
| `0x91`/`0x92`/`0x9B`/`0x9C` firmware, one-way | `0xA1` SetWifiConfig → `0xB1` | `0xA2` GetWifiConfig → `0xB2` | `0xA3` PrintStatus → `0xB3` |
| `0xA5` PrinterStatusData → `0xB5` | `0xA6` Pause → `0xB6` | `0xA7` CompressImage → — | `0xAF` PrinterConfig → `0xBF` |
| `0xC1` Connect → `0xC2` | `0xC3` PrinterFree → `0xC4` | `0xDA` CancelPrint → `0xD0` | `0xDC` Heartbeat → `0xD9`/`0xDD`/`0xDE`/`0xDF` |
| `0xE3` PageEnd → `0xE4` | `0xF3` PrintEnd → `0xF4` | `0xF5` StartFirmwareUpgrade → `0xF6` | |

Printer → client only: `0x00` NotSupported, `0x90` RequestFirmwareCrc, `0x9A` RequestFirmwareChunk, `0x9D` FirmwareCheckResult, `0x9E` FirmwareResult, `0xC6` ResetTimeout, `0xD3` PrinterCheckLine, `0xDB` PrintError, `0xE0` PrinterPageIndex (u16 page).

Payload layouts that matter:

- **`0x40` PrinterInfo** — one sub-type byte, response id `0x40 + subtype`. `1` Density (1 B), `2` Speed, `3` LabelType, `4` PrintheadWidth (2 B), `5` PrintingAccuracy, `6` Language, `7` AutoShutdownTime, `8` **PrinterModelId**, `9` SoftwareVersion (2 B), `10` BatteryChargeLevel (1 B, 0–4 → 0/25/50/75/100 %), `11` SerialNumber, `12` HardwareVersion (2 B), `13` BluetoothAddress (6 B, **reversed**), `14` PrintMode, `15` Area.
- **Model id (`0x48`)** — **branch on payload length, not on firmware version.** 2 bytes → big-endian u16. Exactly 1 byte → `(byte as u16) << 8`. niimbluelib decodes the 2-byte form as a *signed* i16; relabel uses `u16::from_be_bytes` — harmless for every documented id (all ≤ 0x1400) but there is no reason to inherit the sign extension.
- **Serial number (`0x4B`)** — ASCII when the payload is ≥ 8 bytes, otherwise uppercase hex of the first 4 bytes; under 4 bytes means unavailable.
- **`0xA5` → `0xB5` PrinterStatusData** — `[singleColorCache u16][singlePacketSize u16][maxSendSize u16][packetInterval u8][?][supportColor @10][protoVer hi @11][protoVer lo @12][…]`. Valid only when `LEN ≥ 13`.
- **`0xA3` → `0xB3` PrintStatus** — `page u16 BE`, `pagePrintProgress u8` (0–100), `pageFeedProgress u8`. Payloads of 4, 8 and 10 bytes exist; when `LEN == 10`, `data[6]` is an error code and non-zero means the job failed. `data[4]` is an undocumented state byte — a B1 that never started sits with it at `0x02`. relabel surfaces `data[4]` and `data[5]` raw in the protocol log; decoding them would give a far better "why is nothing happening" diagnostic than a timeout.
- **`0xE3` PageEnd / `0xF3` PrintEnd / `0x03` PageStart / `0x20` PrintClear / `0x8E` Calibration** — single reply byte, `0x01` accepted/finished, `0x00` refused/still printing.
- **`0x13` SetPageSize** — six forms, all big-endian u16, **rows (feed axis) first, then cols (printhead axis)**: 2 B `rows`; 4 B `rows, cols`; 6 B `+copies`; 9 B `+cutHeight u16, cutType u8`; 13 B `+0x00, sendAll u8, partHeight u16`; 45 B `+serial[32]`.
- **`0x01` PrintStart** — four forms: 1 B `[01]`; 2 B `[pages u16]`; 7 B `[pages u16, 00,00,00,00, pageColor]`; 9 B `[pages u16, 00,00,00,00, pageColor, speed, flag]`. `pageColor`: 0 SingleColor, 1 DoubleColor, 2 SingleColorAlt, 3 Multiorder. **Picking the wrong length breaks printing.**
- **`0xDC` Heartbeat** — request byte selects the variant: `1` Advanced1 (reply `0xDD`, use when protocolVersion < 3), `2` Basic (`0xDE`), `3` Unknown (`0xDF`), `4` Advanced2 (`0xD9`, protocolVersion ≥ 3).
  - **Advanced2 (`0xD9`, ≥ 9 B)**: skip 2, `chargeLevel`, `temp`, `lidClosed` (0 = closed), `paperInserted` (0 = inserted), `paperRfidSuccess`, `ribbonRfidSuccess`, `ribbonInserted`, then optional `wifiRssi u16`, `_`, `lightingErrorCode`, `voltageState`.
  - **Advanced1 (`0xDD`)** — field offsets selected by payload length: 10 → skip 8, lid, charge; 13 → skip 9, lid, charge, paper, paperRfid; 19 → skip 15, same four; 20 → skip 18, paper, paperRfid.
  - **Lid polarity is inverted** on model ids 272, 273, 274, 512, 513, 514, 1792, 2304, 2560, 3584, 3840, 4352, 5120.
  - Two unresolved conflicts here (see §8): whether the Advanced2 charge byte is the 0–4 enum or a 0–100 percentage, and whether `paperRfidSuccess` is 0-means-ok or 1-means-ok. relabel treats lid/paper/RFID flags as **display-only**, never as a gate on printing, precisely because of this.
- **`0x1A` → `0x1B` RfidInfo** (paper tag) and **`0x1C` → `0x1D` RfidInfo2** (ribbon tag), one layout:
  - **Check the sentinel first: `LEN == 1` means no tag.** Do *not* test `data[0] == 0` the way the Python clients do — `data[0]` is a real UUID byte and a leading `0x00` would false-negative, and if the no-tag byte is ever non-zero the test misses entirely and then indexes past a 1-byte buffer.
  - Otherwise: `uuid[8]` raw (render lowercase hex), `u8`-length-prefixed `barcode` string, `u8`-length-prefixed `serial` string, `allPaper u16 BE`, `usedPaper u16 BE`, `consumablesType u8` (same id space as `LabelType`), then **optional** `capacity u16 BE` only if two bytes remain.
  - Decode the counters as **u16**; niimbluelib uses signed `getInt16`, which is a latent bug. Be lenient about trailing bytes — niimbluelib throws.
  - On many models the printer simply never answers. Time out at ~600 ms and report "no tag information", not an error.
  - `RfidInfo2` on a paper-only printer answers `0x00 In_NotSupported`. Swallow it.
- **`0x59` → `0x69` GetPaperInfo** — protocolVersion ≥ 4 only: `gapHeightPx u16, totalHeightPx u16, paperType u8` (if LEN ≥ 9), `gapHeight u16 /10 mm, totalHeight u16 /10, paperWidthPx u16, paperWidth u16 /10, direction u8, tailLengthPx u16, tailLength u16 /10`.

### 1.7 Connect and version negotiation

1. Open the transport (§3.3).
2. Write `03 55 55 c1 01 01 c1 aa aa`. Wait ≤ 1000 ms for `0xC2`. `data[0]` = ConnectResult: `0` Disconnect, `1` Connected → protocolVersion 0, `2` ConnectedNew → 1, `3` ConnectedV3 → resolve below, `0x5A` FirmwareErrors. **No reply is not fatal**: log, assume version 0, continue. Sleep ~200 ms.
3. If ConnectResult is 3, send `PrinterStatusData 0xA5 [01]` → `0xB5`. With `LEN ≥ 13`: `supportColor = data[10]`, `n = data[11]*100 + data[12]`; **v3** if 204 ≤ n < 300, **v4** if 300 ≤ n < 302, **v5** if n ≥ 302.
4. Read `PrinterInfo 0x40` for sub-codes **08, 0B, 0D, 0A, 07, 03, 0C, 09** in that order, ~600 ms each, tolerating failures individually.
5. Send one `Heartbeat 0xDC` — type `04` if protocolVersion ≥ 3, else `01`.

Steps 3–5 are the "arming handshake" that one hardware-tested driver reports as mandatory on a protocol-3 B1. **[disputed]** Adversarial review established that this is one implementation on one unit, that three independent implementations print to a B1 with no such sequence, and that the reproducible causes of a stalled B1 are (a) unpaced BLE writes and (b) sending `PrintEnd` before status polling reaches completion.

**Resolution.** Do the sequence anyway — they are pure reads and cost under a second — but treat pacing and status-polling as the load-bearing fixes and say so in the code comment. Do not tell a user with a stalled B1 that the handshake is the problem.

Some models answer identification off-spec and the session must degrade rather than abort: the D110 replies to `0xA5` with only 2 bytes (version undeterminable) and answers `0xDC [04]` with `0x00 NotSupported`, burning a full timeout on every heartbeat; the N1 answers `0xB4` instead of `0xB5`.

**Periodic heartbeat.** Not required to print, and it must be **off for the duration of a print job**. relabel's default is a 2000 ms poll while idle on the printer pane, stopped on `print_init` and restarted on job completion — which is exactly what the niimblue app does and what niimbluelib's library layer fails to do. Configurable, including fully off. Nobody has measured how long a NIIMBOT holds an idle BLE link with zero traffic, so this stays a setting rather than a decision.

### 1.8 Image encoding

Everything in this section is verified against a byte-exact reference port that reproduces the wiki's own worked example including the checksum the wiki elides.

**Orientation and polarity.** Row-major, **MSB first**: pixel x=0 is bit 7 (`0x80`) of byte 0; x=7 is bit 0 of byte 0; x=8 is bit 7 of byte 1. **A set bit is black** and fires the thermal element. Rows are padded on the **right** with zero bits to `ceil(cols/8)*8`. (Both Python ports get this wrong — `int(bits, 2).to_bytes(…, "big")` right-aligns, shifting the image right by `(8 - w%8) % 8` px whenever width is not a multiple of 8.)

**Thresholding is the caller's job.** niimbluelib treats any pixel that is not exactly `0xFFFFFF` as black, so an anti-aliased buffer produces fat, blobby glyphs. relabel thresholds or dithers in `relabel-render` and hands the encoder a strict 1-bit plane (§5).

**`0x84 PrintEmptyRow`** — `[row u16 BE][repeat u8]`, `LEN = 3`. Emitted for any row with zero set bits.

**`0x85 PrintBitmapRow`** — `[row u16 BE][C1][C2][C3][repeat u8][stride packed bytes]`, `LEN = 6 + stride`. `row` is the **first** row of the run; `repeat` is the number of rows in the run, so `repeat = 1` means one row.

**`0x83 PrintBitmapRowIndexed`** — same six header bytes, then the black pixels' x-positions as u16 big-endian. Used only when a row has ≤ 6 black pixels. niimbluelib's own comment warns *"Printer powers off if black pixel count > 6"*; one independent driver never uses `0x83` at all and reports the B1 prints correctly without it. **relabel defaults `0x83` off**, behind a setting, because the saving is negligible and the claimed hazard is a power-off.

**The three counter bytes.** Two mutually exclusive encodings, selected by row width against the **printhead**, not the row:

```rust
let chunk = printhead_px / 8 / 3;              // integer division
if chunk > 0 && stride <= 3 * chunk {
    // SPLIT — slices clamped to the row length
    c1 = popcount(&row[0 .. min(chunk, stride)]);
    c2 = popcount(&row[min(chunk,stride) .. min(2*chunk, stride)]);
    c3 = popcount(&row[min(2*chunk,stride) .. min(3*chunk, stride)]);
} else {
    // TOTAL — the u16 sits LITTLE-endian across c2/c3
    let total = popcount(&row);
    c1 = 0x00;
    c2 = (total & 0xFF) as u8;
    c3 = (total >> 8) as u8;
}
```

The thirds are thirds of the printhead. A 240-px full-black row on a 384-px head has `chunk = 16`, giving `C1 = 0x80, C2 = 0x70, C3 = 0x00` — 128/112/0, not 80/80/80. Because `chunk = floor(printhead/24)`, any model whose printhead is not a multiple of 24 — B1 Pro 567, B21 Pro 591, A20/A203/B203 400, A63/B32 851, A1 Pro 296 — **always** falls into total mode for a full-width row. `B21_V1` and `B21_L2B` force total mode regardless. A `printhead_px` of 0 (unknown model) falls into total mode, which is the safe default.

**[disputed]** Whether the counters matter at all. Three implementations hardcode `(0,0,0)` and report success; the wiki says printers "usually" work with zeros; one B21S hardware writeup claims the firmware validates them. **Resolution: compute them correctly.** It is three popcounts per row against a firmware that might check — the cheapest possible insurance. A debug setting forces `(0,0,0)` so a user can bisect a misbehaving model.

**Run merging.** Byte-identical consecutive rows merge into one packet with `repeat` incremented. `repeat` is a u8 and **niimbluelib never caps it**, so a 300-row blank label silently emits `repeat = 44`. **relabel splits runs at 255.** Whether firmware handles a 255-row run correctly is untested; the run length is a constant so it can be lowered to 64 if a model misbehaves.

**Rotation.** Model metadata carries `printDirection`. `top` packs rows straight — canvas width is the printhead axis. `left` **rotates the canvas 90° clockwise at encode time**: `dst(r, c) = src(x = r, y = H - 1 - c)`, so `rows = src.width`, `cols = ceil(src.height/8)*8`, and the canvas *height* becomes the printhead axis. A black pixel at source top-left lands at output top-right.

**relabel's canvas convention**, decided once and applied consistently to presets, preview and `SetPageSize`: **the document is authored in the orientation the printed label reads.** For a `left` model — the 96-px D11/D110/B16/B18 family, A1 Pro at 296, Betty at 192 — that means a landscape canvas whose *height* is the 12 mm label width and whose *width* runs along the feed, which is how a wire-marker label reads. The encoder rotates; the user never does. The label-properties panel labels the two dimensions "across the head" and "along the feed" rather than width/height, so the mapping is never ambiguous.

**`cols`: label width or printhead width? [disputed, unresolved]** niimbluelib sends the label width. The `iscarelli` driver and `kjy00302/niimprint` send the printhead width, and the former claims hardware evidence that a B1 silently drops columns beyond the head when it is given `W = 400` for a 50 mm label — though its own geometry table then lists B1 Pro at 584 px against a 567 px head, so it is not self-consistent.

**Resolution: make the two answers identical.** relabel's raster canvas is always the full printhead width (rounded down to a multiple of 8), the label content is positioned within it, and `cols` is that same number. Both readings then agree and no hardware test is needed to ship. The padding is zero bits, so no element outside the media fires and there is no head-wear concern. A per-model `column_mode: FullHead | LabelWidth` override exists for the model that turns out to disagree, and a hardware test — a full-width black bar sent both ways, comparing the right edge — is on the bring-up checklist.

**Geometry.** `px = mm × dpmm`, `dpmm = 8` at 203 dpi and `11.81` at 300 dpi. The printhead-axis dimension must be a multiple of 8 and must not exceed the model's printhead pixels; relabel rounds **down** and warns, as niimblue does.

### 1.9 Print tasks

Seven sequences, selected by `(model, protocolVersion)`. They differ in `PrintStart` payload width, `SetPageSize` payload width, whether `PageStart`/`PrintClear`/`PrintQuantity` appear at all, and how completion is detected.

| Task | Init | Page | Finish |
|---|---|---|---|
| **D11_V1** (D11, D11S) | SetDensity, SetLabelType, PrintStart **1 B** | PrintClear `0x20`, PageStart, SetPageSize **2 B (rows only)**, PrintQuantity `0x15` u16, rows, PageEnd | passively await unsolicited `0xE0 PrinterPageIndex` until it equals totalPages, then PrintEnd |
| **D110** (D110, B21S, B21S_C2B, D11 at protoVer 1/2) | same, PrintStart **1 B** | PrintClear, PageStart, SetPageSize **4 B**, PrintQuantity, rows, PageEnd | poll PrintStatus until `page == pagesPrinted`, then PrintEnd |
| **B21_V1** (B21) | SetDensity, SetLabelType, PrintStart **1 B** | PageStart, SetPageSize **4 B**, rows with `PrinterCheckLine 0x86` every 200 rows, **counters forced to total mode**, PageEnd; per-copy loop re-sends the page | poll **PrintEnd** until its reply byte is 1 |
| **B21_L2B** | as B21_V1 | PageStart retried up to 5× at 500 ms, PageEnd up to 20× at 500 ms | as B21_V1 |
| **B1** (B1, B1_SE, D110_M, B21_C2B, M2_H, N1) | SetDensity, SetLabelType, PrintStart **7 B** | PageStart, SetPageSize **6 B (rows, cols, copies)**, rows, PageEnd. No PrintClear, no PrintQuantity | poll PrintStatus until `page == totalPages`, then PrintEnd |
| **D110M_V4** (D110_M at v4, D11_H, B21_PRO, B1_PRO, C1, EP1C) | **SetLabelType then SetDensity — reversed**, optional tube/half-cut, PrintStart **9 B** | **one-way PrintStatus, no wait, +~30 ms**, then SetPageSize **13 B**, rows, PageEnd. **No PageStart** | poll PrintStatus, PrintEnd, then **one-way Heartbeat, no wait** |
| **H1S** | PrintStart **2 B** | SetPageSize **9 B** with cutHeight/cutType | poll PrintStatus |

The two D110M_V4 one-way packets are not optional: B21_PRO-class printers **drop the first packet after `PrintStart` and the first after `PrintEnd`**, and those two sacrificial sends are the workaround. The community wiki's D110M_V4 listing shows `SetPageSize` before the one-way `PrintStatus`, which contradicts both the niimbluelib source and the independent driver — **the source order is correct, the wiki listing is wrong.**

`PrintEnd` is what feeds out and retracts the paper. It goes in a `finally` — **it must still be sent on the error path**, or the label is left parked under the printhead. Conversely, polling to completion *before* `PrintEnd` is mandatory rather than an optimisation: `PrintEnd` arriving mid-print produces a cut-off label.

niimbluelib's `waitUntilPrintFinishedByStatusPoll` has no overall deadline and uses strict equality, so a printer that responds but never advances polls forever. **relabel's poll carries a hard job deadline (25 s default), uses `page >= target`, and treats `data[6]` on a 10-byte status payload as terminal.** Completion is judged on the page counter, never on the two progress percentages — `pageFeedProgress` is not reliably 0–100 across models.

**Multi-copy.** The preferred flow uploads the bitmap once: `PrintStart` declares `totalPages = N` and `SetPageSize` carries `copies = N` (B1/V4 tasks) or `PrintQuantity = N` (D11/D110 tasks). The counter climbs 1..N and one `PrintEnd` feeds out. **Exception:** the D110 (id 2304) and N1 (id 3586) acknowledge the request and print only page 1 — the counter parks at 1/100 %/100 %, the next `PageStart` answers `04 00`, and subsequent commands are refused with `0xDB 06`. These need N separate complete jobs. `ModelSpec` therefore carries `pages_per_job: Option<u16>`, and the job engine falls back to N jobs automatically when the counter stalls at 1 with more copies outstanding.

**As built, neither is.** `ModelSpec` has no `pages_per_job`, and there is no stall fallback: `JobError::Stalled` is declared and never constructed, so a printer that capped a job at one page would print one label and then run out the job's 25-second budget. What exists instead is a fixed list, `ONE_PAGE_PER_JOB`, of the D110 (2304) and the N1 (3586), which `job::print` splits into one job per copy; every other printer, the D110_M included, is sent copies as one multi-copy job, which has never run on hardware and whose one budget is shared by every copy. The editor never takes that path: since 12f-2 it sends every copy, on every printer, as its own one-copy job, which is what `relabel-cli label` sends and what has printed on the D110_M (PROTOCOL-NOTES 3b). Print test P11 in RENDER-PLAN is the first check of the multi-copy path on the D110_M.

**Multi-page** (distinct images): `totalPages` = the sum of per-page quantities; `init` once; then per page `page()` + `wait_page()`; then `wait_finished()`; then `end()` in a `finally`. A look-ahead of 2 pages is the validated pipelining depth — 1 loses the race because send time is comparable to print time. A stalled batch must stop pushing pages immediately; continuing produces a short *and* desynchronised job.

`0xDB 06 DataError` in practice means "refused, cannot service now" rather than a malformed packet, and it typically lands on whatever command arrives after the printer considers the job over. relabel surfaces it as "printer refused the command (it may consider the job finished)", not as "data error".

### 1.10 Error codes

`0xDB In_PrintError`, one byte:

`01` CoverOpen · `02` LackPaper · `03` LowBattery · `04` BatteryException · `05` UserCancel · `06` DataError · `07` Overheat · `08` PaperOutException · `09` PrinterBusy · `0A` NoPrinterHead · `0B` TemperatureLow · `0C` PrinterHeadLoose · `0D` NoRibbon · `0E` WrongRibbon · `0F` UsedRibbon · `10` WrongPaper · `11` SetPaperFail · `12` SetPrintModeFail · `13` SetPrintDensityFail · `14` WriteRfidFail · `15` SetMarginFail · `16` CommunicationException · `17` Disconnect · `18` CanvasParameterError · `19` RotationParameterException · `1A` JsonParameterException · `1B` B3sAbnormalPaperOutput · `1C` ECheckPaper · `1D` RfidTagNotWritten · `1E` SetPrintDensityNoSupport · `1F` SetPrintModeNoSupport · `20` SetPrintLabelMaterialError · `21` SetPrintLabelMaterialNoSupport · `22` NotSupportWrittenRfid · `32` IllegalPage · `33` IllegalRibbonPage · `34` ReceiveDataTimeout · `35` NonDedicatedRibbon

`0x00 In_NotSupported` means the printer does not implement the command. relabel treats an unsupported *optional* read (RFID2, GetPaperInfo, half-cut) as "not available" and only treats it as an error on a print-path command.

### 1.11 Timing

Two sets of numbers exist. niimbluelib's library defaults are looser; the hardware-validated driver's are tighter and per-command. relabel takes the per-command set as its defaults, all of them settings:

| | |
|---|---|
| Inter-write pace | **10 ms** (BLE), 0 ms (serial) |
| Default request timeout | 1000 ms |
| SetDensity / SetLabelType / PageStart | 1000 ms |
| PrintStart / SetPageSize | 2000 ms |
| PageEnd | 3000 ms |
| PrintEnd | 2500 ms |
| PrintStatus request / poll interval | 900 ms / 150 ms |
| Overall job deadline | 25 000 ms |
| PrinterStatusData / PrinterInfo (handshake) | 1000 ms / 600 ms |
| Heartbeat request / idle interval | 500 ms / 2000 ms |
| RFID read | 600 ms, absence is not an error |
| B21_L2B PageStart / PageEnd retries | 5 × 500 ms / 20 × 500 ms |

Throughput is dominated by **write count × pace**, not by bytes: roughly 12 ms of wall clock per row write at a 10 ms pace. A dense 5-page B1 Pro batch measures 16.4 s at 177–273 writes per page; light artwork on the same batch, 6.2 s at ~50 writes per page. Frame bundling (§3.3) is the lever if that is not enough.

### 1.12 Deliberately not implemented

Firmware upgrade (`0xF5`/`0x90`/`0x91`/`0x92`/`0x9A`–`0x9E`), image compression (`0x87`/`0xA7`/`0x89`), RFID **writing** (`0x70`), WiFi configuration (`0xA1`/`0xA2`), and the ASCII debug console available over USB in charge mode (`#10001`, `test beep`, `test motor …`). The first three are §9 decisions, not scheduling ones.

---

## 2. Model registry

### 2.1 Source of truth

There is exactly one upstream registry and everyone else copies it: NIIMBOT's own cloud device database, unauthenticated at `https://print.niimbot.com/api/hardware/list` and mirrored at `https://oss-print.niimbot.com/public_resources/static_resources/devices.json` (~160 KB, 80 models). niimbluelib's `printer_models.ts` is *auto-generated* from it, and the community wiki's model table is generated from it by a separate script — so those two are not independent confirmations, they are the same data twice.

**relabel vendors a dated snapshot of `devices.json` into the repo and generates its table from that at build time.** Not a network fetch at build time (unreproducible builds, offline breakage) and not a copy of `printer_models.ts` (inherits an attribution chain and the EU sui-generis database-right question for no benefit). The generator is a `build.rs` in `relabel-proto` emitting a `const [ModelSpec]` array plus a `phf`-free linear-scan lookup — 80 entries, a scan is free.

Field mapping:

| JSON field | `ModelSpec` |
|---|---|
| `codes` | `wire_ids: &[u16]` — the DeviceType the printer reports |
| `paccuracyName` | `dpi: 203 \| 300` |
| `widthSetEnd` | `print_width_mm`, and `printhead_px = ceil(widthSetEnd × ppmm)`, ppmm = 8 or 11.81 |
| `maxPrintWidth` | `media_width_mm` — sanity-clamp against `widthSetEnd`; B2_PRO, EP2M_H and K4 have upstream data-entry errors here |
| `maxPrintHeight` | `max_label_mm` (50–350) |
| `printDirection` 0 / 90 / 270 | `Top` / `Left` / `Left` |
| `paperType` CSV | `label_types: &[LabelType]` |
| `solubilitySetStart/Default/End` | `density: RangeInclusive<u8>` + default |
| `rfidType` 0/1/2/3 | none / label / ribbon / both |
| `isSupportCalibration` | gates `0x8E` |
| `interfaceType` 1/2/3/4 | `vendor_hint` — logged, never used to route logic |
| `errorPrintStrategy` | `unrecognised_paper: Normal \| LowDensity2 \| Forbid` — advisory UI text only |

**[resolved]** `interfaceType` partitions off 11 non-NIIMBOT devices (B3 = Zicox; B11/B50/B50W/JC_M90/S1/S3/T6/T7/T8 = Daozhen, wire codes 51457–51718, the only models with density 6–15; T2S = SNBC). Adversarial review argued it was merely an OEM key, on the grounds that `interfaceType = 1` alone spans all seven print flows and no open-source implementation branches on it.

A first-party reading contradicts that review: the vendor SDK carries separate factories and separate print-task implementations for the Zicox, Daozhen and SNBC lines, next to a framing class named for NIIMBOT alone. `interfaceType` **is** a protocol-family key, and the numbers do not run in a guessable order — 1 is Gengcon's own line, 2 Zicox, 3 Daozhen, 4 SNBC, fixed by the registry's own `interfaceName` field.

**Resolution: record it, key model detection off `codes`, select the print flow from the model and the negotiated protocol level, and warn on the 11 rather than refusing them.** Nobody has reported trying one with a third-party client, and a clear failure is more useful than a pre-emptive block.

### 2.2 Identification and its traps

- **Model ids are not unique.** `256` → A8 / B3S / JCB3S; `512` → D11 / HI_NB_D11; `2305` → D110 / HI_D110. Lookup returns the first match, and for 512 the two candidates differ (96 px vs 120 px head). The device pane shows the matched name with an "or …" affordance to pick the other.
- Three catalogue entries (D41, D61, DXX) carry **no wire id at all** and can never be identified.
- **B1 and B1 Pro advertise the same BLE name.** The model must be read from the printer, never inferred from the advertisement.
- niimbluelib (derived) and `hass-niimbot` (hand-entered) **disagree on printhead pixels for about a dozen models**, including D11_H (142 vs 178), K4 (832 vs 656) and B18/N1/D110_M (96 vs 120). niimbluelib itself changed D11_H from 178 to 142 at some point. Only a full-width print on real hardware settles these. `ModelSpec` carries a `printhead_px_confidence: Derived | Measured` flag and the UI offers a manual override.
- `B21_H` (784) appears in one third-party registry and neither in the vendor DB nor niimbluelib. Provisional.

### 2.3 Unknown-model defaults

1. Read `protocolVersion`. **≥ 5 → D110M_V4 sequence; otherwise → B1 sequence.** This is the field-tested fallback, and B1 is the single best default — it is what people are told to try first and it works on D101, D11_H, B3S, M2, N1 and B1_PRO.
2. `printhead_px = 0`, which forces total counting mode. **Do not guess a width.** Cap the image and warn.
3. dpi 203, `printDirection: Top`, density 3 clamped to 1..5, `LabelType::WithGaps`, no RFID, no calibration, no test page, single colour.
4. **Clamp density to the model's own range.** A hard-coded 1–5 slider is wrong for a large minority: the Daozhen block is 6–15, T2S is 1–20, the 300 dpi B32/Z401/A63/T8S line is 1–15, the 96-px D11 family is 1–3, ET10 is fixed at 3.

**The user can always override the print task.** The niimbluelib mapping does not always match what users report working; a dropdown listing the seven tasks, defaulting to the table's answer, is a one-line UI cost that saves an unprintable model.

### 2.4 Colour

Almost everything is monochrome. Red/black is a **paper** property: the vendor DB lists dual-colour thermal paper for exactly **B1_PRO (4097), B21_PRO (785), B2_PRO (6912)**. B31, B3S_A and B3S_P take paper that develops red instead of black — single-colour, no protocol change. Despite its name **B32R is not red-capable** in this sense; its red comes from a ribbon.

On the wire, colour is `PageColorType::DoubleColor = 1` in `PrintStart` plus `0x8A PrintBitmapRowDoubleColor` with `BitmapColorMode { Empty 0, Red 1, Black 2, Mixed 3 }`. Nobody in any issue tracker has posted a successful two-colour print. The runtime `supportColor` byte (`0xB5` `data[10]`) is the safer gate but its exact semantics are undocumented.

**relabel's document model carries a per-element colour of `Black | Red` from day one** — retrofitting a colour plane into a bitmask document is painful and this costs nothing now — but two-colour *printing* is deferred past v1 and gated on `supportColor`.

---

## 3. Crate layout

Four crates plus a development CLI. rpgp's core/gui split, extended where a boundary buys something specific.

```
relabel/
├─ Cargo.toml                 workspace, resolver 3, edition 2024, rust-version 1.92
├─ crates/
│  ├─ relabel-proto/          bytes in, bytes out. No I/O, no async, no GUI.
│  ├─ relabel-render/         document model → 1-bit plane. No printer, no I/O.
│  ├─ relabel-core/           transports, session, print tasks, job engine. Async.
│  ├─ relabel-gui/            Slint. Binary `relabel`.
│  └─ relabel-cli/            headless driver for hardware bring-up. Not shipped.
├─ vendor/devices.json        dated snapshot, regenerated by hand
└─ fuzz/                      cargo-fuzz targets for the decoder
```

**Why four and not two.** `relabel-proto` is pure and synchronous, which makes it fuzzable, table-testable against captured hex, and usable from a `no_std`-ish context later. `relabel-render` has a heavy dependency tail (parley, tiny-skia, resvg) that neither the protocol nor a headless print server needs. Keeping them apart means the fuzz target compiles in a second and the protocol tests need no fonts.

### 3.1 `relabel-proto`

Frame codec, opcodes, enums, payload builders and parsers, the image encoder, the model registry, the error taxonomy. Depends on `thiserror` and nothing else.

```
src/
  frame.rs        encode/decode one frame; Encoder returns Err on LEN > 255
  accumulator.rs  the streaming reassembler of §1.4
  command.rs      RequestId / ResponseId (two enums), the correlation table
  payload.rs      typed builders: print_start(), set_page_size(), …
  parse.rs        typed parsers: PrinterStatus, RfidTag, Heartbeat, PaperInfo
  error.rs        PrinterErrorCode, ProtoError
  image.rs        Plane -> Vec<Frame>, run merging, counters, rotation
  models.rs       include!(concat!(env!("OUT_DIR"), "/models.rs"))
build.rs          vendor/devices.json -> models.rs
```

The image encoder's signature is deliberately free of any printer object:

```rust
pub struct HeadSpec {
    pub printhead_px: u16,           // 0 = unknown, forces total counting mode
    pub direction:    PrintDirection,
    pub counts:       CountsMode,    // Auto | ForceTotal | ForceZero (debug)
    pub indexed_rows: bool,          // 0x83; default false
    pub max_run:      u8,            // default 255
}

pub fn encode_page(plane: &BitPlane, head: &HeadSpec)
    -> Result<EncodedPage, ProtoError>;

pub struct EncodedPage { pub rows: u16, pub cols: u16, pub frames: Vec<Frame> }
```

### 3.2 `relabel-render`

The label document, its serialisation, and the rasteriser. Knows a *target* — width in dots, dpi, direction — and nothing else about printers.

```rust
pub struct Label { pub size: LabelSize, pub elements: Vec<Element> }
pub enum Element { Text(TextEl), Barcode(BarcodeEl), Qr(QrEl),
                   Image(ImageEl), Rect(RectEl), Line(LineEl) }

pub struct Target { pub width_px: u16, pub height_px: u16, pub dpi: u16 }

pub fn rasterise(label: &Label, t: &Target) -> Pixmap;                 // 8-bit grey
pub fn binarise(p: &Pixmap, m: Binarise) -> BitPlane;                  // 1-bit
pub enum Binarise { Threshold(u8), FloydSteinberg, Atkinson, Bayer(u8), … }
```

`BitPlane` is the boundary type between render and proto: `{ w: u16, h: u16, bits: Vec<u8> }`, MSB-first, stride `w/8`. It lives in `relabel-proto` so `relabel-render` depends on proto and not the reverse.

### 3.3 `relabel-core` — transports and session

**The transport trait is dumb bytes.** Reassembly, pacing, correlation and the request mutex all live above it, once, so a new transport is forty lines.

```rust
#[async_trait]
pub trait Transport: Send {
    /// One write. May contain one or several complete frames.
    async fn write(&mut self, buf: &[u8]) -> Result<(), TransportError>;
    /// Raw inbound bytes. Never frame-aligned. Taken once, by the session.
    fn take_reader(&mut self) -> Option<mpsc::UnboundedReceiver<Vec<u8>>>;
    /// Largest single write. BLE: negotiated MTU - 3. Serial: usize::MAX.
    fn max_write(&self) -> usize;
    /// Default inter-write gap. BLE 10 ms, serial 0.
    fn default_pace(&self) -> Duration;
    fn kind(&self) -> TransportKind;
    async fn close(&mut self) -> Result<(), TransportError>;
}
```

That is the trait as built. The reader is taken rather than borrowed because the session owns it for the life of the connection, and the trait asks only `Send`, not `'static`. `close` is called only by `Session::close`, which waits behind the commands already sent: dropping a session does not close its transport, so a program that outlives its sessions, as the editor does, closes each one. The command line leaves that to its exit.

Three implementations:

**`BleTransport` (btleplug 0.13).** Connect path, with every landmine handled:

```rust
// ScanFilter only filters by advertised service UUID, which NIIMBOT does not
// advertise — and on Linux BlueZ merges discovery filters across all D-Bus
// clients anyway. Always scan wide and post-filter on name.
central.start_scan(ScanFilter::default()).await?;

peripheral.connect().await?;
peripheral.discover_services().await?;

// Try the known pair first, then fall back to a capability scan. The known pair
// is not NIIMBOT-specific (the same stock BLE-UART bridge appears on unrelated
// hardware) and it is not universal — B3S is reported to use "uncommon UUIDs".
let ch = find_known(&peripheral)
    .or_else(|| find_by_capability(&peripheral))     // non-SIG service,
    .ok_or(NoChannel)?;                              // NOTIFY + (WNR or WRITE)

// notifications() BEFORE subscribe(): on Windows and macOS this is a tokio
// broadcast channel of capacity 16 whose Lagged errors are *silently dropped*,
// and anything delivered before the stream exists is lost outright.
let notes = peripheral.notifications().await?;
peripheral.subscribe(&ch).await?;
tokio::spawn(drain_into_unbounded(notes, tx));       // never let 16 slots lag;
                                                     // aborted on close and on drop

// mtu() unwraps an Option<u16> on the BlueZ backend and panics when BlueZ does
// not publish the characteristic MTU property (older than ~5.62).
let mtu = catch_unwind(AssertUnwindSafe(|| peripheral.mtu())).unwrap_or(23);
```

Known-good pair: service `e7810a71-73ae-499d-8c15-faa9aef0c3f2`, characteristic `bef8d6c9-9c21-4c9e-b632-bd58c1009f9f`. Both are exposed as user-overridable settings.

**Write type — [disputed].** Every niimbluelib-lineage client uses write-without-response exclusively. `hass-niimbot` defaults to **write-with-response** for command packets and deliberately interleaves acked writes during bitmap transfer as flow control, keyed on an EMA of write RTT. Whether write-with-response is even permitted is per-model: one driver reports the characteristic as WRITE_NO_RESPONSE-only on B1/B1 Pro. **Resolution: discover the properties at runtime, prefer `WithoutResponse`, expose a `Paced | Acked` flow-control setting, and never assume either.** On macOS specifically, using write-with-response on a write-without-response characteristic is reported to silently wreck timing and print blank labels — so the fallback is gated on the property actually being present.

**Pacing — [disputed].** The claim that 203 dpi B1-class printers *require* ~10 ms and 300 dpi models tolerate an unpaced burst rests on a single lab's document, is uncorroborated, and is internally inconsistent in that document. What is corroborated is that unpaced BLE writes drop rows on some host/adapter combinations, and that all three BLE implementations pace **every** model at 10 ms with no per-model exception. **Resolution: a 10 ms transport-level default, settable to 0, not keyed on model or dpi.**

**MTU and bundling.** Every model's row frame fits one write at the commonly-negotiated ~247 MTU (largest is ET10 at 213 bytes) and **none fits at the 23-byte default**. relabel never splits a frame across writes — that direction is undocumented and untested. Instead: if `max_write < largest_frame`, refuse to print with a clear message naming the MTU. Bundling several complete frames into one write up to `max_write` is documented as safe and gives roughly 4× throughput; it ships behind a setting, default off until it is tested on all three platforms.

**Identity.** On macOS `Peripheral::address()` returns an all-zero `BDAddr`. Persist the `PeripheralId` and the name, and reconnect with btleplug 0.13's `Central::retrieve_peripherals` — which works without scanning on all three platforms and is the right primitive for "remember my printer". Except on Windows: there, retrieval by identifier leaves out a device that is not connected (btleplug-0.13.0 `winrtble/adapter.rs:207-219`), so it cannot find a printer that has dropped its link, and a Reconnect built on it would need a scan to fall back on. Nothing is remembered yet (§6.1).

**`SerialTransport` (serialport 4.10).** CDC-ACM at **115200 baud**, same frames, same fragmentation. Confirmed on D110, D11_H, B3S, B21_C2B and B21_PRO (which enumerates as VID:PID `3513:0002`; no public VID/PID list exists for the rest, so the port is user-picked or probed by handshake). A dedicated blocking reader thread feeds the same unbounded channel — `tokio-serial`/`mio-serial` buys nothing here and adds a dependency. Field-reported caveat for D11_H over USB: skip the info handshake, disable heartbeat, and add **no** artificial inter-packet delay — even 5 ms breaks its parser. That is why pace is a transport default and not a constant.

**Bluetooth Classic: not a first-class transport in v1.** btleplug refuses Classic and will not add it; there is no production-grade cross-platform RFCOMM crate (`bluer` is Linux-only, `bluetooth-rust` omits macOS and is immature, `bluetooth-serial-port` and `io_bluetooth` are dead). But it falls out mostly free: **on Windows and macOS, pairing an SPP device makes the OS create serial nodes** (a COM port pair; `/dev/cu.*` and `/dev/tty.*`), so `SerialTransport` already covers it if the user pairs in OS settings. Document that. Linux is the odd one out — `rfcomm bind` is deprecated in BlueZ 5.x — and a raw `AF_BLUETOOTH`/`SOCK_STREAM`/`BTPROTO_RFCOMM` socket on channel 1 is about forty lines with `socket2`, added only if a Classic-only model turns up. Note that each printer has **two** Bluetooth addresses, LE and BR/EDR, differing by a rotation of the first three bytes; only the Classic one advertises the SPP SDP UUID.

### 3.4 `relabel-core` — session, tasks, jobs

```rust
pub struct Session {
    transport: Box<dyn Transport>,
    acc:       Accumulator,          // relabel-proto
    events:    broadcast::Sender<PrinterEvent>,   // unsolicited packets
    gate:      Mutex<()>,            // one outstanding request, always
    pace:      Duration,
    trace:     TraceSink,            // bounded ring of hexdumps for the UI
}

impl Session {
    pub async fn request(&self, p: Packet, expect: ResponseId, t: Duration)
        -> Result<Packet, SessionError>;   // short-circuits on 0xDB and 0x00
    pub async fn send_oneway(&self, p: Packet) -> Result<(), SessionError>;
}

#[async_trait]
pub trait PrintTask: Send {
    fn id(&self) -> PrintTaskId;
    async fn init(&mut self, s: &Session, o: &PrintOptions) -> Result<()>;
    async fn page(&mut self, s: &Session, p: &EncodedPage, copies: u16) -> Result<()>;
    async fn wait_page(&mut self, s: &Session) -> Result<()>;
    async fn wait_finished(&mut self, s: &Session) -> Result<()>;
    async fn end(&mut self, s: &Session) -> Result<()>;   // always runs
}

pub fn select_task(model: Option<&ModelSpec>, proto: u8, override_: Option<PrintTaskId>)
    -> Box<dyn PrintTask>;
```

The job engine owns retries, the deadline, the copies/pages fallback for D110-class printers, cancellation (`0xDA CancelPrint`, `0xA6 Pause`), and progress reporting. It is transport-agnostic and model-agnostic; everything model-specific is either in `ModelSpec` or behind `PrintTask`.

**As built,** cancellation is not in it. `job::print_labels`, the editor's loop of one-copy jobs, reads a stop flag before each label, so a run stops between labels and the label in hand always reaches its `PrintEnd`; nothing interrupts a label once started, and neither `0xDA` nor `0xA6` is ever sent. A cancel inside a label waits for print test P10 in RENDER-PLAN, a paper test of what those two commands do mid-page, and for a relabel-core change of its own that makes the job cooperatively cancellable (DESIGN-GAPS #11).

### 3.5 `relabel-gui` and `relabel-cli`

`relabel-gui` follows rpgp's `ui/` layout exactly: `theme.slint`, `widgets.slint`, `dialogs.slint`, `app-window.slint`, `types.slint`. The GUI depends only on `relabel-core`, `relabel-render` and `relabel-proto` types; no `btleplug::` type reaches a Slint callback. As built, that holds through `relabel-core`'s `link` module, the GUI's only way to a printer, whose API names no tokio or btleplug type; `relabel-gui` depends on neither crate, and its `tests/deps.rs` checks the source and the manifest for either. `transport::ble` still hands btleplug types to the command line.

`relabel-cli` is a workspace member excluded from release builds: `scan`, `info`, `status`, `rfid`, `print <png>`, `raw <hex>`, `dump`. It is how milestone 1 gets tested and how every hardware question in §8 gets answered, on all three test machines, without the GUI existing.

---

## 4. Dependencies

`[workspace.dependencies]`, exact versions, one line of justification each. The comments in the real manifest explain *why the feature set is what it is*, per rpgp convention.

| Crate | Version | Features | Why |
|---|---|---|---|
| `thiserror` | 2 | | Error enums in every crate; no runtime cost. |
| `anyhow` | 1 | | GUI and CLI only — cores return typed errors. |
| `btleplug` | **0.13** | | The only maintained cross-platform BLE central in Rust. 0.13 is the first release where CoreBluetooth reports the *negotiated* MTU and routes write-without-response through a FIFO honouring `canSendWriteWithoutResponse` — exactly the backpressure a row-streaming printer depends on. `mtu()` itself only arrived in 0.12. |
| `tokio` | 1 | `rt-multi-thread`, `sync`, `time`, `macros` | btleplug requires a tokio runtime; the same worker-thread-plus-event-loop pattern rpgp already uses. |
| `futures-util` | 0.3 | | Notification streams. |
| `serialport` | 4.10 | `default-features = false` | USB CDC, and Bluetooth Classic for free on Windows/macOS via OS-created serial nodes. Default `libudev` links C libudev on Linux; dropping it costs only USB VID/PID metadata in enumeration, which no NIIMBOT publishes anyway. **MPL-2.0** — file-level copyleft, fine to link from MIT, recorded in `THIRD-PARTY.md`. |
| `socket2` | 0.6 | *(Linux, deferred)* | Only if a Classic-only model turns up; raw RFCOMM channel 1. |
| `slint` | 1.18.1 | `default-features = false` + `std`, `compat-1-2`, `accessibility`, `backend-winit`, `renderer-femtovg-wgpu`, `renderer-software`, `unstable-wgpu-29` | Identical to rpgp, for identical reasons: `backend-default` compiles in Qt whenever `qmake` is on PATH and then prefers it at runtime, and `renderer-femtovg` is OpenGL, deprecated on macOS. |
| `slint-build` | 1.18.1 | | Build dependency. |
| `tiny-skia` | 0.12 | | Pure-Rust 2D rasteriser, BSD-3-Clause, no C. **Already compiled** in a Slint 1.18 build via resvg → usvg, so it costs zero new crates. |
| `parley` | **0.11.1** | | Shaping, line breaking and font fallback. The same 0.11.1 Slint 1.18 compiles, so the graph holds one copy of parley, parley_data, fontique and harfrust, and wherever Slint is built alongside, its system-font backends, `complex-scripts` and `accesskit` reach relabel-render's copy; relabel-render asks none of them (see `Cargo.toml`). Re-exports fontique, so no separate dependency. |
| `skrifa` | **0.47** | | Glyph outlines into `tiny_skia::PathBuilder` via `OutlinePen`. Newer than the 0.44 Slint 1.18, parley 0.11 and swash share, so the graph holds two copies; no skrifa type crosses the parley boundary. |
| `image` | 0.25.10 | `default-features = false`, `png`, `jpeg`, `bmp`, `gif`, `webp` | Import of user artwork, decoded once and stored as a grey PNG. Not for dithering: `imageops::colorops::dither` panics on anything narrower than two pixels (RENDER-DESIGN §4.7). Pure Rust for every one of these — only `avif-native` drags in libdav1d; `webp` is `image-webp`, pure Rust. |
| `resvg` | 0.48.1 | `default-features = false` | SVG artwork, rendered at import only. Slint 1.18 compiles the same 0.48.1 for `@image-url`, so there is one copy, and in the GUI build Slint's `text`, `raster-images` and `svgz` are on in it. relabel-render asks for none of them and reaches none: SVG text is refused by name, no image is loaded, and usvg is handed a parsed document. Built alone it leaves `text` and `raster-images`, whose decoders ignore `image`'s limits, off; `roxmltree` comes with usvg regardless. |
| `barcoders` | 2.0 | | Code128 A/B/C, EAN-13/8, UPC-A, Code39, Code93, Code11, ITF/STF, Codabar. `encode()` returns a `Vec<u8>` of raw 0/1 modules with no rendering attached — exactly what a device-pixel blitter wants. Low activity but not abandoned. |
| `qrcode` | 0.14 | `default-features = false` | Zero transitive dependencies with defaults off. Its defaults pull `image`, `svg`, `pic`, `eps`, none of which we want. |
| `serde` | 1 | `derive` | The `.relabel` label document. |
| `serde_json` | 1 | | Document format, and the `devices.json` parse in `build.rs`. |
| `dirs` | 7 | *(deferred)* | Config, saved printers, recent labels. Deferred to the slice that adds settings or recent files: until then a file dialog starts in the current file's folder or at the platform's own default. Take 7.0.0 then, which fixes the Windows `preference_dir` trap RENDER-DESIGN §2.4 records for 6, and admit the MPL-2.0 `option-ext` it brings through `dirs-sys`, which `deny.toml` refuses today. |
| `rfd` | 0.17.2 | defaults (`xdg-portal`, `wayland`) | File dialogs: the xdg-desktop-portal on Linux, IFileDialog on Windows, a sheet on macOS. Nothing is compiled from C: on Linux it loads libdbus-1 with `dlopen` at run time and runs zenity when there is no portal. `gtk3` stays off, for its nine C `-sys` crates and a licence `deny.toml` refuses. Only the async API, inside `slint::spawn_local`, parented through Slint's `raw-window-handle-06`. |
| `open` | 5 | | Hands a URL to `xdg-open` / `open`. |
| `arboard` | 3 | `default-features = false`, `wayland-data-control` | Slint exposes no clipboard API. |
| `yeslogic-fontconfig-sys` | 6 | `dlopen`, Linux/FreeBSD only | **Not used directly.** Named so feature unification turns `dlopen` on for fontique's copy, which removes the build-time `pkg_config::find_library("fontconfig").unwrap()` from the whole graph. The top-level `slint` crate re-exports no feature that does this, so a direct target-conditional dependency is the only manifest-level route. |
| `winresource` | 0.1 | Windows build dep | Icon, version info and a DPI-aware manifest in the `.exe`; without it the resource section is empty. |
| `libfuzzer-sys` / `arbitrary` | — | `fuzz/`, not a workspace member | Decoder fuzzing. |

**No logging crate.** A protocol app's most valuable diagnostic is a hexdump of the last N frames with direction and timestamp, visible *in the app* when a user reports "it acks everything and nothing prints". That is a bounded ring in `Session` rendered by a Protocol Log pane — better than `env_logger` for the actual failure mode, and one fewer dependency.

**No `crc32fast`.** Only firmware needs CRC-32, and firmware is out of scope.

**What is not pure Rust**, for the README table:

| Library | Via | Why |
|---|---|---|
| `libdbus` | `btleplug` → `dbus 0.9` (Linux) | BlueZ is a D-Bus service. `dbus/vendored` builds it from source via `cc` if a pure link is preferred. |
| `libfontconfig` | `fontique` (Linux) | **Loaded at runtime via `dlopen`**, not linked — see the dependency note above. With no libfontconfig present, system-font enumeration is empty, which is one more reason to embed the label fonts. |
| `libdbus` | `rfd` (Linux) | **Loaded at runtime via `dlopen`** by the file-chooser portal backend: the same library `btleplug` links, not a second one. |
| `zenity` | `rfd` (Linux) | An external program, run only when the desktop has no file-chooser portal. With neither, no file dialog opens: a label is opened, or a new one started at a path, from the command line. |
| `libudev` | avoided | `serialport` with `default-features = false`. |
| `libwayland` | `winit` | Loaded at runtime on a Wayland session. |

No `capnp` compiler, no SQLite, no OpenSSL. relabel's build is materially cleaner than rpgp's.

---

## 5. The rendering pipeline

Verified end to end by a working proof of concept that renders a 384×320 B21 label and produces a correct 15360-byte plane.

```
Label (document)
  → parley 0.11 layout  →  skrifa 0.47 outlines  →  tiny_skia::PathBuilder
  → barcoders / qrcode  →  fill_rect at integer module size, anti_alias = false
  → image / resvg       →  scaled blit
  = tiny_skia::Pixmap (premultiplied RGBA)
  → luma (0.299/0.587/0.114)
  → threshold or dither (Floyd–Steinberg, Atkinson, Bayer 2/4/8, plain)
  = BitPlane, MSB-first, right-padded to a byte
  → relabel-proto::image::encode_page
  = Vec<Frame>
```

**Anti-aliasing is a per-element decision, not a global one.** `tiny_skia::Paint` defaults to `anti_alias = true`. Barcode bars, QR modules, hairlines and rectangles **must** set it false — a half-lit module is a scan failure. Text keeps anti-aliasing and is thresholded, which reads best at 28 px on a 203 dpi head; whether that flips at 8–10 pt on a 96-dot head is an open question for a side-by-side print (§8).

`tiny-skia` 0.12 introduced `Paint::colorspace`, defaulting to `Linear`, which gamma-corrects blending and therefore changes which anti-aliased edge pixels survive a fixed threshold. The threshold default is calibrated against a printed reference, not guessed.

**Dithering** is hand-rolled — the two crates that exist are a 2021 release and a 73-download 0.1.0. Kernels, all directly portable: Floyd–Steinberg denom 16 `[[0,0,7],[3,5,1]]`; Jarvis–Judice–Ninke denom 48; Stucki denom 42; Burkes denom 32; Sierra-3 denom 32; Atkinson denom 8 `[[0,0,0,1,1],[0,1,1,1,0],[0,0,1,0,0]]` with `normalize = false` so only 6/8 of the error diffuses. Serpentine scanning on by default, threshold default 128.

**Fonts are embedded**, registered through `fontique::Collection::register_fonts`. A label must rasterise identically on every machine, and with fontconfig dlopen'd there may be no system fonts to enumerate on Linux anyway. System fonts are additionally offered where available.

**Preview into Slint**: `slint::Image::from_rgb8(SharedPixelBuffer<Rgb8Pixel>)` built straight from the 1-bit plane (bit → `0x00`/`0xFF` triple). A 384×320 label is 123 KB and about 0.1 ms. The `Image` element gets `image-rendering: pixelated` so the operator sees real printer dots rather than a blurred scale-up. rpgp has no precedent for this — it uses only compile-time `@image-url` SVGs — so `SharedPixelBuffer` is new ground here.

Re-render is debounced (~80 ms) and runs off the event loop. At 1229 dots wide a full re-render on every keystroke will not stay interactive; incremental/dirty-region rendering is deferred until measured.

---

## 6. The UI

Five panes in a left-rail shell, in the order a user meets them. Slint files mirror rpgp: `theme.slint`, `widgets.slint`, `dialogs.slint`, `app-window.slint`, `types.slint`.

### 6.1 Devices

The first screen on a cold start. Two lists and a manual entry.

- **Bluetooth** — a live scan list: name, signal, and "seen 2 s ago". Scanning is wide (`ScanFilter::default()`) with a name post-filter and an "show all devices" escape, because NIIMBOT name prefixes span most of the alphabet and are near-useless as a filter. Each row: **Connect**. A previously-used printer pins to the top with a **Reconnect** that goes through `retrieve_peripherals` without scanning.
- **Serial** — enumerated ports with a note that a Bluetooth-Classic printer paired in OS settings appears here on Windows and macOS.
- **Manual** — a UUID pair override for the models with uncommon characteristics, and a raw port path.

Connection progress is shown as the actual handshake steps (Connect → status → info ×8 → heartbeat), not a spinner, because that is where a stall happens and the user should see which step it stalled on. On failure, the specific step plus a link to the Protocol Log.

Permissions get first-class messages rather than an opaque error: macOS needs `NSBluetoothAlwaysUsageDescription` in the bundle (a TCC usage string, not an entitlement — so rpgp's hardened-runtime, no-entitlements-file signing script stays valid), a CLI outside a bundle needs the terminal itself added under Privacy → Bluetooth, Linux needs no group membership for BLE GATT but does need `dialout`/`uucp` for USB serial, and unpackaged Win32 needs no manifest capability at all.

**As built, in 12f-1 (RENDER-PLAN step 12).** There is no Devices pane yet. Connect is the Preview pane's default button (§6.4). It scans once, for four seconds, wide and filtered on the name prefixes, with no "show all devices" escape yet; a single printer found is opened without asking, several are offered as up to four buttons in the same pane, those heard in the scan first, and none found is said. The handshake is shown as its steps, on a line in the pane that a screen reader does not announce, and a failure names the step it stopped at in plain words; there is no Protocol Log to point at. A connect gives up at 15 seconds. Bluetooth that is off, missing, unreachable or not allowed each has its own sentence, the macOS one saying where to allow it. The live list, signal and last-seen, show all devices, Serial, Manual and Reconnect wait for this pane, and nothing is remembered between runs.

### 6.2 Printer

What is connected and what it is doing.

- **Identity** — model name (with the "or …" disambiguator for the ambiguous ids), wire id, serial, firmware and hardware version, Bluetooth address, protocol version, selected print task with an override dropdown.
- **State** — battery (0–4 → percent, honestly labelled "approximate"), lid, paper, RFID success flags, head temperature where the Advanced2 heartbeat provides it. All display-only, none gating a print, because the polarity and units are disputed (§8).
- **Consumables** — the RFID tag read-only: barcode/SKU, serial, total and used counts, consumable type, capacity if present. "No tag" and "printer did not answer" are distinct states with distinct wording. A short explanation that an unrecognised tag usually means near-zero density rather than a refusal, and that relabel never writes tags.
- **Settings** — density (slider clamped to the model's own range, with the range shown), label type, print mode, speed, auto-shutdown, sounds, volume. Each writes its command and reports the reply; an unsupported value comes back as `0xDB 0x1E`/`0x1F`/`0x21` and is surfaced as "this printer does not support that value", not as a failure.
- **Actions** — Print test page (`0x5A`, gated on model support), Calibrate (`0x8E`, gated on `isSupportCalibration`, with the warning that on a B1 it ejects roughly 15 cm of paper without retracting it), Reset settings (`0x28`), and a Protocol Log toggle.

**As built, in 12f-1.** Only a Printer line, plain text beside the File bar: the printer's name and model, its head in dots and its dpi, the firmware, and the battery in words, leaving out whatever the printer did not say. An id the registry does not know is said as "a printer relabel does not know", with the id, and one it gives to several models that differ, such as id 2305, as "a D110 or a Hi-D110": named, not guessed at. The pane itself, with the D110_M warning of §8.4, waits.

### 6.3 Designer

A canvas and an inspector. Deliberately modest: this is a label composer, not Illustrator.

- **Label properties** — the New label pane, from the File bar or Control+N. Two dimensions labelled **across the head** and **along the feed**, never width/height. Across the head is a read-only readout of the whole print head in millimetres and dots, and stays one while nothing draws media narrower than the head (`Target::require_head`). Along the feed is one millimetre field, to a tenth, showing its dot count and refusing a length past the printer's longest label. The presets are the registry's default label for the model and, where that stock is 12 mm across, the measured 40 × 12 mm roll: `devices.json` has no media list to drive more. The printer is the connected printer's model when relabel is connected to one it can print on, and otherwise the open label's model, or a D110_M, said to be assumed, and assumed because the connected printer is one relabel cannot make a label for when that is the reason; a 300-dpi head that is not whole bytes is not offered.
- **Canvas** — the full printhead width at 1:1 dots with a zoom, showing the media area and the padding beyond it as distinct tones so the user can see where the paper ends. Snap-to-dot, arrow-key nudge by one dot.
- **Elements** — Text, Barcode, QR, DataMatrix, Rectangle, Line, added from a toolbar or with Control+Shift and a letter, resized by grips on the element's far edges or by the inspector's lengths, and deleted with Delete, Backspace or a toolbar button. An Image is added only by importing its artwork, from the toolbar's Image… or with Control+Shift+I, since an image without artwork draws nothing. Select on canvas, edit in the inspector.
- **Inspector** — per type, and the primary keyboard path: every field is a named, announced, arrow-steppable text box, with drag on the canvas as the accelerator. Common to all seven element types: name, feed, head, feed length, head length, rotation in degrees, ink, and a read-only row for the element's state. Text: content (a template), font family, em size in dots, line height, alignment, weight, rasterisation. Barcode: symbology, data, module width in dots, bar height. QR: data, module size in dots, error-correction level, pinned version. DataMatrix: data, module size, symbol size. Image: source path, artwork (Import… or Import again…, a button beside a summary of the artwork), fit, halftone, threshold. Rectangle: stroke. Line: thickness. The document has no field for human-readable text, a quiet-zone override, a wide:narrow ratio, letter spacing, wrap, vertical alignment or shrink-to-fit, so the inspector offers none: see RENDER-DESIGN §7.4 for what was sketched against what was built. A value this build cannot draw is kept exactly as it was typed and said out loud, because the file format promises to round-trip it; a live validity indicator is the pane that shows preflight's own issues, not this one.
- **Data** — a CSV/column binding for batch printing, `{column}` placeholders in text and barcode data, with a row-count readout. This is what turns the app from a toy into a tool and it belongs in the design from the start even if it ships in M4.

### 6.4 Preview and print

Split view: **rendered** (greyscale, pre-threshold) beside **printed** (the actual 1-bit plane at 1:1 with `pixelated`). Both are needed — the greyscale is how you judge a photo before choosing a dither, the 1-bit is what actually goes to the head — and showing only one is how people ship blobby labels.

Print settings: copies, density (defaulting to the model's own default), label type, dither mode and threshold with a live preview, and an advanced disclosure holding pace, write type, bundling, counters mode, indexed rows and print-task override. Advanced settings are visibly non-default when changed, so a support conversation starts from the truth.

A **Bytes** readout — frames, total bytes, estimated seconds at the current pace — because the estimate is `write_count × pace` and users deserve to know why a dense label takes 16 seconds.

**As built, in 12f-1: Preview, not yet print.** Control+P, or Preview on the File bar, opens a Preview pane with the printed half only: the 1-bit plane a job would encode, with `pixelated` scaling at a whole number of pixels to the dot, or reduced by whole dots to the pixel where it would not fit, drawn for the label's own printer and, once one is connected, checked against its head, dpi and direction and never re-fitted to them. Beside it are a verdict, preflight's issues with the refusals first, and every reason the editor would not print it: a label that prints the date or time or takes CSV rows, which the command line prints, an unknown or ambiguous printer, another printer's head, and a link too narrow for a row. The grey, pre-threshold half is deferred, together with the dither and threshold controls it exists to judge and the editor's artwork import, which is what would need them. The Bytes readout is there, as a facts line of frames, bytes and seconds, at the command line's 12 ms a frame rather than a pace setting, which does not exist yet. None of the print settings is built: copies and Print come in 12f-2, and the pane's command is called Preview until then.

**As built, in 12f-2: Print.** The pane's command is now Print, and it prints. Of the print settings, copies are built: a Copies field from 1 to 99, defaulting to 1 and kept for the life of the window, each copy sent as its own one-copy job. Density and label type are shown read-only, in the facts line and in Print's description, as the values a print is sent with - the model's own default density and its first label type, the formula `relabel-cli label` uses, which on the D110_M is density 3 on labels with gaps. The rest waits: a density and label-type control waits for the Printer pane's settings (§6.2) and a reason to override the model's defaults; dither mode, threshold and the grey half wait for artwork import in the editor, which is what they exist to judge; and pace, write type, bundling, counters mode, indexed rows and the print-task override have no settings to show, since none is configurable from the editor. The Bytes readout stays the facts line's frames, bytes and seconds. RENDER-PLAN step 12, "As built, print (12f-2)", has the detail.

**As built, in 12g: dither mode and threshold, per image.** Artwork import is built, so dither mode and threshold are too, as per-image settings rather than print settings: the document has no page-wide one, the page being always `Zone::PAGE`, threshold at 128, and each image carries its own halftone and threshold, which import chooses and the inspector edits. Their live preview is the canvas, whose one-bit plane draws every image with its own halftone and threshold after every edit, and a change is also said in words - how much of the image's frame now prints, and how much did before, or for an image in white ink how much it knocks out - which is how it is judged without looking. The grey, pre-threshold half is deferred again, to 12h. The renderer's side of it exists, but the print pane's image box, at most 640 by 200 logical pixels, needs a second image with a name and a description of its own and a layout check at every scale, and putting artwork on a label does not need it. Until then the grey is judged only as far as the canvas's one-bit plane and the ink share allow, which is the weaker check for a photograph: that is the risk the split view exists to remove, and 12h's to close. RENDER-PLAN step 12, "As built, images (12g)", has the detail.

**As built, in 12h: the grey half.** The split view is built, for the labels it exists for: those with dots that print otherwise than their greys look - artwork under a halftone or a threshold that changes it, and anti-aliased text. The pane shows the grey, pre-threshold drawing under the 1-bit plane, both `pixelated` at one whole-number scale, the same size and with their left edges aligned, captioned Dots and Greys. A label whose greys are already its dots, as every black-and-white label in the test corpus is - hinted text, codes and shapes - shows the plane alone, in a box that now takes a 100 mm label at one pixel to the dot. The pane is up to 920 px wide, against 700 before. Dither mode and threshold stay per image in the inspector, as 12g built them, and the pane changes nothing; an assistive technology's change there while the pane is open is proved again and shown in both halves. The grey image is named and described in words, by how many dots differ, the average ink of their greys and how many of them print. RENDER-PLAN step 12, "As built, the grey half (12h)", has the detail.

### 6.5 Job

Modal-free progress: current page of total, the printer's own page counter, the two progress percentages (labelled as the printer's own numbers, since `pageFeedProgress` is unreliable), elapsed against the deadline, and a live frame counter. **Stop after this label** ends a run between labels, so the label in hand always finishes and `PrintEnd` always runs; a cancel inside a label waits for print test P10, since what `0xDA` does to paper position mid-page is unknown (DESIGN-GAPS #11). Errors are plain words: one sentence naming the printer, giving the cause, how many labels printed and whether the one in hand may be part printed, then what to do - never a command byte or an error type's name, a fault code relabel does not know being said by its number alone, and `0xDB 0x06` said as a refused command and a label the printer never reported printed pointing at the roll and the cover.

The Protocol Log pane sits behind all of this: last N frames, direction, timestamp, decoded name, hexdump, with copy-to-clipboard. It is the single best bug report a user can send.

**As built, in 12f-2.** A job line in the print pane: a progress bar whose value is labels printed over labels asked for, and beside it, not live, which label is being sent and the printer's own two percentages; the Printer line says which label of how many is printing. Stop after this label is the job's only control. Deferred, each for a reason: the printer's page counter as a figure of its own, because with one job per label it only ever counts to one; elapsed time against the deadline, because each label has its own 25-second budget and a budget that runs out is said in words, so a countdown would be a figure the operator can do nothing with; a live frame counter, because the rows go in one bulk send that reports nothing until it is done; and the Protocol Log, which waits for its own pane. Every job failure, fault code and refusal is mapped to words (DESIGN-GAPS #19); RENDER-PLAN step 12, "As built, print (12f-2)", has the table.

---

## 7. Milestones

Ordered, smallest useful thing first. Each one is testable.

**M0 — `relabel-proto`, no hardware.** Frame codec, accumulator, both enums, the correlation table, payload builders and parsers, the image encoder, the `devices.json` generator. Table tests against every verified hex example in §1. Fuzz target on the accumulator. Exit criterion: the D11 wiki row example, the two `PrintEmptyRow` examples and the Connect frame all encode and decode byte-exactly, and the encoder rejects a 256-byte payload.

**M1 — `relabel-cli` prints a PNG.** `BleTransport`, `Session`, the B1 print task, `scan`/`info`/`status`/`print`. One model, one transport, one task. Exit criterion: a PNG comes out of a real printer on Linux, then on macOS and Windows via the test VMs. This is where every hardware question in §8 gets its first answer, and where the `cols` and counters experiments run.

**M2 — `SerialTransport`.** Same session, same task, different byte pipe. Gives a permission-free CI path and a fallback for uncommon-UUID models. Exit criterion: the same PNG over USB.

**M3 — GUI shell: Devices + Printer.** Slint app, scan, connect, identity, state, settings, RFID read, Protocol Log. No designer yet — printing is still a file path from the CLI. Exit criterion: connect, read everything, change density, disconnect, reconnect from saved.

**M4 — `relabel-render` + Designer + Preview + Job.** Text, barcode, QR, image, rect, line; dithering; the split preview; the job pane. This is the first release anyone else would want. Exit criterion: compose and print a label without touching a file.

**As built.** The exit criterion was met in code by 12f-2 and on hardware on 2026-09-25. Images and per-image dithering are met in code by 12g, which imports artwork into the editor and edits each image's halftone and threshold; their prints on the D110_M are owed. The split preview is met in code by 12h (§6.4), which shows the grey drawing under the dots wherever the two differ, so M4's list is complete in code; the prints of art labels on the D110_M that 12g owes are still owed.

**M5 — Model and task breadth.** D110, D11_V1 and D110M_V4 tasks; the copies/pages fallback for D110-class printers; the print-task override; the `printhead_px` override. Exit criterion: at least one printer from each of the four task families prints.

**M6 — Packaging and release.** Flatpak, macOS bundle with `NSBluetoothAlwaysUsageDescription` and hardened-runtime signing, Windows `.exe` with `winresource` — and, learning from Azzurro's `0xC0000135`, an actual run of the published binary on a clean Windows VM before anyone is told it works. `SHA256SUMS` plus a detached signature, verified as a stranger would.

**M7 — Batch/CSV, B21_V1, B21_L2B, H1S, two-colour.** Everything the earlier milestones deliberately left out.

Not on the list at all: firmware upgrade, RFID writing, compression, WiFi.

---

## 8. Risks and unknowns

Honest inventory. Each item says how it gets resolved.

### 8.1 Things a hardware session settles — the M1 bring-up checklist

1. **`cols`: label width or printhead width?** relabel sidesteps it by making them equal (§1.8), but the experiment is cheap and worth doing: print a full-width black bar with `cols = label_width` and again with `cols = printhead_width`, compare the right edge. If they differ, `column_mode` becomes load-bearing.
2. **Do the counter bytes matter?** Send a row with deliberately wrong `C1/C2/C3` and see whether it is rejected. Determines whether the split/total logic is load-bearing or cosmetic.
3. **Negotiated ATT MTU per platform and per model.** Only one source gives a number (~247, on B1/B1 Pro). If any model sits at 23, single-frame writes break for everything above a 96-px head and the protocol layer needs a fragmentation strategy nothing in the wild exercises. Measure with `Peripheral::mtu()` on Linux/macOS/Windows.
4. **Does btleplug's `WriteType::WithoutResponse` reproduce Web Bluetooth's timing** on BlueZ, CoreBluetooth and WinRT? The macOS failure mode for the wrong write type is silent — a blank label reported as 100 % — so this is checked visually, not by return code.
5. **Is 10 ms pacing needed at all on the target hardware?** Bisect down from 10 to 0 on each model.
6. **Does the printer accept one frame split across writes?** Bundling is documented as safe; splitting is not. Test only so the failure is known, then never do it.
7. **Multi-copy per job.** Which models beyond D110 (2304) and N1 (3586) silently cap at one page? The failure acks everything and parks the counter at 1/100 %/100 % until a timeout, so it needs a real N-copy job per model.
8. **Anti-aliased-then-thresholded text vs hard-edged**, at 8–10 pt on a 96-dot head. Side-by-side print. Also settles whether the threshold needs to be gamma-aware given `tiny-skia`'s linear blending.
9. **Printhead widths where niimbluelib and hass-niimbot disagree** — D11_H 142 vs 178, K4 832 vs 656, B18/N1/D110_M 96 vs 120. A full-width bar measures it.
10. **What a printer does with a bad `LEN` or a bad XOR.** No source documents whether it drops silently, resets its parser, or answers `0x00`. Worth knowing before relying on any recovery behaviour.

### 8.2 Things no experiment we can run will settle

- ~~**The compression codec** behind `0x87`/`0xA7`.~~ **Settled: it is miniLZO**, which ships as a native library in both the vendor SDK and the vendor app. Still unknown: which models require it, and what `GetCompress` returns. relabel never compresses regardless — run-merging plus `0x84` collapses realistic label content well enough, and those opcodes carry a wider length field whose checksum width is unspecified.
- **`PrinterStatusData` `data[4]` and `data[5]`.** `data[4] = 0x02` correlates with "job never started"; `0xDA` has been seen at `data[5]`. Decoding these would give a far better stall diagnostic than a timeout.
- **The Advanced2 heartbeat charge byte** — 0–4 enum or 0–100 percent? niimbluelib types it as the enum; the wiki's own worked example has that byte at `0x64` (100). One of the two is wrong.
- **The Advanced2 RFID flags' polarity.** The wiki annotates paper RFID as "0 = ok" and ribbon as "1 = ok" in the same diagram; niimbluelib parses both as `!= 0`.
- **The temperature byte's units.** Never stated. The example value `0x4d` = 77 is implausible as an idle print-head temperature in °C.
- **Advanced1 heartbeat layout keying** — niimbluelib selects by payload length (10/13/19/20), the wiki says the official SDK selects by model id (17/20/26/27). Those differ by exactly the 7-byte frame overhead, which is suggestive but unconfirmed.
- **`setPrinterTime` payload** — 9 bytes `[0x08, 0x01, year u16, …]` per niimbluelib, or 8 bytes `[0x01, year u16, …]` with the `0x08` acting as the length byte per the wiki. Needs a wire capture.
- **Undocumented opcodes**: `0xAF PrinterConfig`, `0x70` (labelled "GetVolumeLevel / WriteRFID", possibly two features on one opcode), `0x0B AntiFake`, `0x1E`'s field breakdown, `PrinterInfo` sub-type `0x0F Area`, `0x09`, `0x0D`, `0x10`, `0x14`, `0x1F`, `0x54`, `0x05`, `0xDA`'s payload, and whether `0xC6 In_ResetTimeout` requires any client action.
- **Whether the 11 non-`interfaceType:1` models can be driven at all.** Nobody has reported success with B3, T2S, or the Daozhen block. They are plausibly TSPL/ESC-POS devices. relabel warns rather than refuses.
- **Whether a periodic heartbeat prevents an idle disconnect.** Nobody has measured how long a NIIMBOT holds a link with zero traffic, or whether heartbeats refresh the auto-shutdown timer.

### 8.3 What vendor-source analysis settled

This section originally listed what a decompile of the official app would
answer. That work has since been done — see `PROTOCOL-NOTES.md` for the method
and the provenance. What it produced:

**Settled.**

1. **`cols` semantics.** The vendor sends `min(image_width, printhead_px)`, with
   the printhead figure a per-model constant. That is why two working clients
   appeared to contradict each other; both were right for what they had tested.
   This was the highest-risk open item in the whole specification.
2. **The per-model print sequence.** Selection is by model family **and**
   negotiated protocol level together. The D11/D110/D101 family uses a one-byte
   `PrintStart` at levels 0 and 1 and a two-byte one at level 2 — **never the
   seven-byte form**, which the most widely copied open-source table wrongly
   assigns to D101. B1 below level 3 also uses the two-byte form. §1.9's table
   has been corrected accordingly.
3. **The compression codec** is miniLZO.
4. **Model capabilities.** The manufacturer's own device registry is vendored at
   `vendor/devices.json`: wire ids, resolution, print widths, density ranges,
   print direction, RFID type, supported label types, and the firmware versions
   on which a given model's RFID read is broken.
5. **`interfaceType` is a protocol-family key**, not merely an OEM label — see
   §2.1.
6. **Confirmations**, which matter more than they sound. The
   reverse-engineering literature looks like seven agreeing implementations but
   is two lineages plus a wiki by one of their authors; a first-party reading is
   a genuine third. It confirmed the frame layout, rows-before-columns in
   `SetPageSize`, the `0x84` payload order, the `0x83`/`0x84`/`0x85` row
   semantics, and the inverted-lid model set.

**Not settled by it, and not settleable by it.** Vendor source shows what the
app *sends*, never what the firmware *enforces*. So the counter-byte question,
Cancel and Pause behaviour mid-page, the printhead-width disagreements between
third-party tables, and the heartbeat units and polarities all remain hardware
questions — they are in §8.1 and §8.2, where they belong.

None of this ever blocked M1–M4.

### 8.4 Non-protocol risks

- **btleplug is unproven for this hardware.** No NIIMBOT project uses it in production; the one Rust attempt that does hardcodes characteristic UUIDs (`0000FF00`/`FF01`/`FF02`) that contradict every hardware-validated source and subscribes before opening the notification stream. M1 is deliberately a btleplug spike before anything else is built on it.
- **D110_M has a documented pattern of spontaneous bricking.** relabel shows a warning on that model. This is not a relabel bug and must not become one. Not yet: the warning waits for the Printer pane (§6.2), and 12f-1, which connects to a D110_M and identifies it, shows none.
- **`serialport` is MPL-2.0.** File-level copyleft, fine to link from MIT, recorded in `THIRD-PARTY.md` rather than discovered later.
- **Slint's royalty-free terms require attribution**: its `AboutSlint` widget in an About screen reachable from the top level, or its attribution badge on the download page. The licence names the widget, so a line of text naming Slint is not enough; `THIRD-PARTY.md` has the detail. Bundled fonts carry the SIL OFL text beside them.
- **No existing Rust crate is usable.** The one crate on crates.io (`niimprint-rs`, MIT, 24 downloads) does not compile at all on macOS or Windows — an ungated `bluer` import means the platform stub is unreachable dead code — and implements a single print flow that matches no niimbluelib task cleanly. The two GitHub-only attempts are GPL-3.0 and license-ambiguous respectively. `relabel-proto` is written from scratch against the facts, with small pieces optionally lifted from MIT sources under attribution.

---

## 9. Licence and legal posture

MIT, like rpgp. Three tiers govern what anyone working on this may read and reuse, and everyone should know which tier a browser tab is in before reading it.

**Tier 1 — facts, free from any source including GPL ones.** Byte values, opcodes, UUIDs, command sequences, DPI, printhead pixel counts, packet layouts. Interoperability information is not copyrightable, and reverse engineering for interoperability is protected in the US (17 USC §1201(f); *Sega v. Accolade*) and the EU (Software Directive Art. 6). Cite the URL in a code comment.

**Tier 2 — code we may copy, with attribution.** MIT: `MultiMote/niimbluelib` and `niimblue` (© 2024 MultiMote), `kjy00302/niimprint` and `AndBondStyle/niimprint` (© 2023 kjy00302), `shellixyz/niimprint-rs`, `eigger/hass-niimbot` (© 2025 eigger), `iscarelli/niimbot-web-bluetooth`, `talaviram/libreniim`. Apache-2.0 sources exist too but carry NOTICE and patent terms — prefer an MIT source where both cover the same ground. Every copied file keeps its copyright line and gets an entry in `THIRD-PARTY.md`.

**Tier 3 — read for understanding, never copy or transliterate.** GPL: `labbots/NiimPrintX`, `EvilGeniusLabs-ca/Thermalith` and its `Niimbot.Net` package, `wiktorpyk/niimbot-rs`, `vookimedlo/vooki-thermo-printer`, `punzik/niimbot-go`, `ooguz/niimbot-printer`, `belu33fr/niimbotlabels`. **No licence at all is stricter than GPL**: `MultiMote/niimblue-node` (confirmed — no LICENSE file, no `license` field), `hanMaster/niim-rs`, `dtgreene/niimbotjs`, `ayufan/niimprint-web`, `MarkusOderSo/niimgo`, `anibaldeboni/go-niimbot`. If someone has read Thermalith's C# closely, they implement that area from the wiki or the hass-niimbot docs, not from memory of the code.

**The community wiki is CC-BY-SA-4.0.** Facts are free; prose and tables are not. Pasting a wiki table into our documentation would put that file under share-alike. Restate and cite — which is what §1 of this document does throughout.

**niimbluelib's README carries a non-commercial warning. That is not a licence term** — its LICENSE file is plain MIT — and it does not bind us. It also must not be copied into relabel's README as though it were one.

Four things relabel stays out of, deliberately:

1. **RFID is read-only, always.** Reading the tag for label type and counts is ordinary interoperability and is what every client does. Writing tags, cloning them, or resetting the print counter is where §1201 arguments live. The wiki documents the roll tag as TEA-encrypted and redacts the key; relabel does not go looking for it. The documented workaround — moving a tag onto the printer case — is a fact users will find anyway and relabel neither implements nor promotes.
2. **No firmware.** Never bundle, mirror, or link vendor firmware.
3. **Trademark stays nominative.** "NIIMBOT" is the vendor's mark. "relabel" is clean; keep the mark out of the name, the icon and any domain, use it only as "works with NIIMBOT printers", and carry a not-affiliated disclaimer.
4. **Region lock is not a feature.** It is a server-side check the official app makes against the printer serial; third-party clients are unaffected simply by not making the call. That is worth one factual sentence in the README and must never be marketed as "unlocks region-locked printers".

No DMCA notices mentioning NIIMBOT exist in GitHub's public repository, and no cease-and-desist, lawsuit or takedown against this ecosystem has been found. It has been public since 2023 and every major repository is live. That is the baseline relabel should not be the project to disturb.

---

## Appendix — disputed claims ledger

Every place the research disagreed with itself, and what relabel does. Kept together so a future maintainer can re-litigate any one of them without re-reading the underlying research.

| # | Dispute | relabel's choice |
|---|---|---|
| 1 | Is the `0x03` Connect prefix protocol or convention? | Send it (write-side only, per-command flag, settable). Never special-case a leading `0x03` on receive — `PageStart` *is* `0x03`. Missing `0xC2` is not fatal. |
| 2 | Is Connect required at all? | No. Four implementations never send it and print. It is the only route to version negotiation, so send it anyway. |
| 3 | Is the B1 post-connect handshake mandatory? | Unproven — one implementation, one unit. Do it (cheap reads), but treat pacing and status-polling as the real fixes and say so in the comment. |
| 4 | Do the `C1/C2/C3` counters matter? | Compute them correctly. Three popcounts against a firmware that might check. Debug toggle forces zeros. |
| 5 | `cols` = label width or printhead width? | **Settled: neither on its own.** The vendor sends `min(image_width, printhead_px)`, which is why two working clients appeared to contradict each other. relabel sends the image width and refuses a page wider than the head rather than truncating it. |
| 6 | Is ~10 ms pacing model-specific? | No. Transport-level default of 10 ms on BLE, 0 on serial, settable to 0. Not keyed on model or dpi. |
| 7 | Write-with-response or without? | Discover properties at runtime; prefer `WithoutResponse`; expose `Paced \| Acked`. Never assume. |
| 8 | Is a periodic heartbeat required? | No, and it must be **off during a print**. The *in-sequence* one-way heartbeat after `PrintEnd` on D110M_V4 **is** required. |
| 9 | `interfaceType` as a protocol-family key? | **Yes** — the vendor SDK has separate factories and print tasks per OEM line. Record it, warn on the 11 non-type-1 models, but key everything off `codes` + protocol level. |
| 10 | Model id 1-byte form = old firmware? | No — branch on payload **length**, not firmware. And decode as `u16`, not niimbluelib's signed `i16`. |
| 11 | RFID "no tag": `LEN == 1` or `data[0] == 0`? | `LEN == 1`. The byte test false-negatives on a leading-`0x00` UUID and over-reads a 1-byte buffer. Also: no answer at all is a normal third state. |
| 12 | RFID counters signed or unsigned? | `u16`. |
| 13 | `0x1E` response id, `0x11`/`0x12` mapping | Accept both candidates in each case. Neither is on a print path. |
| 14 | `setPrinterTime` payload 8 or 9 bytes? | Unresolved. Not implemented in v1. |
| 15 | Does `0x83` indexed-row mode help or hazard? | Default off. The claimed hazard is a printer power-off; the saving is negligible; one driver never uses it and prints fine. |
| 16 | `PrinterCheckLine 0x86` — client-sent or printer-sent? | Both exist. Client-sent every 200 rows on B21_V1/B21_L2B only; printer-sent `0xD3` is an unsolicited received-row count that relabel logs and, later, may use as a genuine drop check. |
| 17 | Heartbeat layout by length or by model id? | By length, as niimbluelib does — but every flag it yields is display-only, never a gate. |
| 18 | Advanced2 charge: enum or percent? Flag polarity? Temp units? | Unresolved. Displayed with hedged labels; never blocks a print. |
| 19 | Which Slint text-stack crates come from `default`? | `parley 0.10`, `fontique 0.10`, `skrifa 0.42`, `harfrust 0.8` do. `swash`, `zeno` and the second `skrifa 0.44` arrive via the software renderer, femtovg and the compiler — not via `shared-parley`. Pin parley/skrifa regardless; that is what unification requires. Since Slint 1.18, relabel-render's parley 0.11.1 is Slint's own again; its skrifa 0.47 is not. See `Cargo.toml`. |
| 20 | Can `niimprint-rs` be reused? | No. It does not compile on macOS or Windows at all, and its single print flow matches no niimbluelib task. Write `relabel-proto` from scratch. |