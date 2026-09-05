# Open questions and gaps

An adversarial read of `DESIGN.md`. That document is strong on **bytes** and close to empty on **building an application**. What follows is real gaps only, each checked rather than assumed, prioritised, with a way to close each one.

Items marked *(settled)* have since been answered — see `PROTOCOL-NOTES.md`. They are kept rather than deleted so the reasoning stays on the record.

---

## P0 — Contradictions in the specification that will break printing

**1. `cols` = label width or printhead width? Two dimensions assert opposite answers, both marked CRITICAL.**
`lifecycle` states flatly: *"The `cols` value must be the PRINTHEAD pixel width, not the label width — the printer prints columns 0..printhead-1 and silently drops the rest"* (high/CRITICAL). `imagedata` claims the opposite (niimbluelib sends label width) and files it as `[low/CRITICAL]` with "sources DISAGREE", then punts to hardware. Nobody noticed these two dimensions contradict each other. This is the single most dangerous unresolved item in the pack — get it wrong and every label is truncated or offset.
→ *Resolve from source before touching hardware*: read `image_encoder.ts` `encodeCanvas()` and every `print_tasks/*.ts` call site to see what value is actually passed to `setPageSize*b`, and cross-read `iscarelli/niimbot-web-bluetooth/src/niimbot.js`. If they genuinely differ, that is a *behavioural* difference between two working clients and needs a print test with a full-width black bar on the target model.

**2. The 0x8a double-colour packet layout was never read from source.** `commands` gives two mutually exclusive shapes in one sentence (`[colorMode, subCmd(0x83/0x84/0x85), …]` **or** `[0x03, row u16, repeat, mask+color bytes]`). The actual signature is `printBitmapRowDoubleColor(blackData, redData, pos, repeats, cols)` — a black plane plus a red plane, building mask and colour byte arrays. Neither description in the specification matches that cleanly, and neither quotes the body.
→ Fetch `packet_generator.ts` (the function body), `image_encoder.ts` double-colour path, and `D110MV4PrintTask.ts`. Decide whether v1 ships two-colour at all (probably not — three models, zero reported successful prints).

**3. The Connect `0x03` prefix is asserted as a critical protocol fact in five places and demolished by the adversarial review.** That review concluded it is a write-side niimblue-lineage *convention*, that four independent implementations omit it and print fine, and that a receive-side decoder special-casing a leading `0x03` will mis-parse (PageStart *is* command 0x03). The headline prose was never corrected to match.
→ No further research needed. Write one sentence of doctrine into relabel's PROTOCOL notes ("send it, never parse it, never assume it") and delete the contradictory prose.

**4. Are the three counter bytes load-bearing?** Three dimensions raise it; none resolves it. The only evidence for "firmware validates them" is one blog post about a B21S — and nobody checked whether that author's blank labels had a different cause (he also describes an inverted-bitmap attempt, i.e. he was changing several things).
→ Cheap experiment on real hardware: print identical rows with (a) correct split counts, (b) all-zero, (c) deliberately wrong non-zero. Until then implement niimbluelib's logic exactly (correct is never worse) and log a warning rather than branching.

**5. There is no conformance-test plan.** One Python reference encoder is the entire golden-vector story. Nothing asks how `relabel-core` gets tested without a printer on CI, or which capture material is reusable.
→ Inventory what exists: `iscarelli/niimbot-web-bluetooth/docs/NOTES.md` (~59 KB of dated hardware captures, MIT), the wiki's worked hex examples (facts, freely usable), and a reference Python port. Build a `tests/vectors/*.txt` corpus from those, plus a `MockTransport` replaying recorded printer responses. There is **no public btsnoop/pcap corpus for NIIMBOT anywhere** — captures must be produced, not found.

---

## P1 — Whole subject areas with zero coverage

**6. Runtime/threading architecture: not researched at all.** Nothing asks how a Slint UI, a tokio runtime, btleplug, 10 ms write pacing, a heartbeat timer, unsolicited packets and a **Cancel** button coexist. rpgp already carries `tokio { features = ["rt-multi-thread"] }`, but its async is short request/response HTTP — not a multi-second streaming device session with live progress. Slint's own guidance is explicit that a current-thread tokio runtime cannot share the Slint event-loop thread; the pattern is tokio on its own thread + `invoke_from_event_loop` back.
→ Read slint-ui discussions #7246, #5784, #12510 and `slint::spawn_local` docs; then a spike: printer session as an actor owning the transport, `mpsc` commands in, progress events out, `invoke_from_event_loop` to update the UI. Decide *now*, because it determines the shape of `relabel-core`'s API (it must not be Slint-aware — per rpgp convention).

**7. How does the user learn the label size?** Genuinely unanswered and it is the *first* thing a user hits. *(confirmed as a real gap, in practice)* A first print underfilled a roll by 15 mm because nothing on the wire says how long a label is. The workaround that worked was printing a millimetre ruler on the label itself and reading off where the printable area ends; a preset table plus that trick is probably the shipping answer. RFID gives a barcode SKU but **not** dimensions; the printer never forwards the tag's dimensions; `GetPaperInfo (0x59)` is protocol-v4+ only; the SKU→mm lookup is a **cloud** endpoint (`getCloudTemplateByOneCode`) that relabel deliberately will not call. So on a v3 B1 with a genuine roll, relabel knows nothing about the paper.
→ Needs a product decision plus research: ship a preset table of common NIIMBOT roll sizes keyed by SKU (harvest from the vendor DB / niimblue presets), let the user measure, or offer an optional opt-in cloud lookup. Check whether niimblue ships a label-preset list and where it came from.

**8. The label document model and file format — one passing mention, no research.** niimblue's `ExportedLabelTemplate` is concrete and I confirmed it: `{ canvas: <fabric.js serialization {version, objects}>, label: LabelProps, thumbnailBase64?, title?, timestamp?, id?, csv?: {data} }`, and `LabelProps` carries `printDirection, size{width,height}, shape (rect|rounded_rect|circle), split (none|vertical|horizontal), splitParts, tailPos, tailLength, mirror (none|copy|flip)`.
→ Two facts the `rendering` dimension missed entirely: (a) **split labels, tails and mirroring are first-class label geometry** in the incumbent tool and are not in relabel's rasterizer design; (b) there is a **`csv` field — variable-data/merge printing** is an expected feature. Decide interop (import niimblue JSON? fabric.js object model is a heavy thing to reimplement) and define relabel's own schema. Also unresearched: where files and settings live (rpgp uses `dirs`).

**9. Packaging and OS integration for a *Bluetooth* app: not asked.** rpgp already ships `packaging/{app.rpgp.rpgp.yml (Flatpak), cargo-sources.py, homebrew/, winget/, macos-sign.sh, icons.sh}` and CI `ci.yml/release.yml/checksums.yml`. The specification covers runtime *permissions* well but never asks what changes in that machinery: does Flatpak's offline `cargo-sources.py` vendoring cope with btleplug's `dbus/vendored` cc build? Does the macOS `NSBluetoothAlwaysUsageDescription` survive the existing `macos-sign.sh` flow, and does a **Homebrew cask** get a bundle (needed for TCC) or a bare binary (which cannot prompt)? Does winget/unpackaged Win32 need anything? Does the Linux user need `dialout` and how is that surfaced?
→ Read rpgp's `packaging/` and `release.yml`, then diff against BLE/serial requirements. This is a shipping blocker, not a nicety.

**10. Disconnect, reconnect and recovery mid-job: absent.** What state is the printer left in if BLE drops after `PageStart` but before `PageEnd`? Does a reconnect need `PrintEnd`/`PrintClear` to unwedge it? Does `In_ResetTimeout (0xC6)` matter here (flagged "no handler exists" and never chased)? A GUI *will* hit this; the specification covers only the happy path plus `PrintEnd`-in-a-finally.
→ Ask the field: mine `MultiMote/niimblue` issues and `niimbluelib` issue #1 for "stuck", "hangs", "won't print until power cycle". Design a "printer is wedged → power-cycle" diagnostic.

**11. Cancel / Pause are unimplemented everywhere and unresearched.** `CancelPrint 0xDA → 0xD0` and `Pause 0xA6` exist; niimbluelib does not use them; nobody has tested what they do to paper position, whether they are safe mid-page, or whether `0x05 UserCancel` comes back. A desktop GUI needs a Stop button that is not "close the app".
→ Hardware test once a printer exists; until then design the Stop button as "stop sending rows + `PrintEnd`" and say so.

---

## P2 — Model/protocol coverage glossed over

**12. WiFi / network transport: named as a fact, never researched.** `models` says B4_PRO, K3_W, MP3K_W have WiFi and B4_PRO is "5GHz/MQTT", and `commands` lists `0xA1/0xA2 Set/GetPrinterConfigurationWifi` — then nothing. My own search turned up no third-party WiFi/MQTT work at all; the vendor materials describe B4 as Bluetooth. Either it is out of scope (say so explicitly in relabel's README) or it is an undiscovered protocol surface.
→ Fetch the wiki's B4/B4 Pro hardware pages and `other/niimbot-cloud.md`; search Chinese-language sources. Cheap, and closes a "does relabel support my WiFi printer" question permanently.

**13. The 11 non-NIIMBOT-protocol models. *(settled)*** The adversarial review held there was no primary-source evidence those models speak a different protocol, and that routing on `interfaceType` was wrong. A first-party reading has since contradicted the review: the vendor SDK carries separate factories and separate print tasks for the Zicox, Daozhen and SNBC lines, alongside a NIIMBOT-specific framing class. `interfaceType` **is** a protocol-family key. relabel records it and warns; it does not refuse.
→ Search "B50 TSPL", "芝柯 B3 打印 协议", SNBC T2S SDK. Then either refuse with a *reason* or just let them fail with a clear error.

**14. Firmware upgrade: fully documented, scope never decided.** Two dimensions detail the CRC32 framing, chunk size and handshake in loving detail. Nobody asks whether relabel should implement it — next to a claim that D110_M has "a documented pattern of spontaneous bricking".
→ Decide: **no**, and write it down. Implementing a flasher against a reverse-engineered protocol is the one feature that turns a bug report into a dead printer.

**15. Compression (0x87/0xa7) is open in three dimensions and nobody searched for the codec.** All three say "undocumented, no implementation". Nobody looked at the official Android SDK/jar, Chinese blogs, or `GetCompress 0x89/0x88` semantics.
→ One targeted search pass (Chinese sources, `JCAPI`/`JCPrinter` SDK docs). If it stays unknown: never emit, treat inbound as unparseable, and note the speed ceiling.

**16. Printhead-width disagreements left as "hardware settles it" with no interim policy.** ~12 models differ between niimbluelib (derived) and hass-niimbot (hand-entered), including D11_H 142 vs 178 and K4 832 vs 656 — a 2× error in one case. Which table ships in v1?
→ Policy decision + a user-visible override, plus a "report your measured width" path. Related: the model catalogue is quoted as 79 / 80 / 77 / 81 rows in four different dimensions and never reconciled; and the "generate from `devices.json` at build time" recommendation was never checked for endpoint stability, versioning, UA/CORS gating, or offline builds — Flatpak and reproducible CI builds cannot fetch. **Vendor a pinned snapshot.**

---

## P3 — Verification hygiene and things a GUI needs that nobody asked about

**17. "N independent sources agree" is repeatedly false and inflates confidence.** `framing` opens with *"Seven independent implementations agree exactly"*. They are two lineages: kjy00302 (→ AndBondStyle → NiimPrintX → go/kotlin ports) and niimbluelib (→ niimblue → niimblue-node → iscarelli, who says "same as niimblue" in his own comments) — plus the wiki, same author as niimbluelib. Thermalith and libreniim are the only arguably-third readings. The adversarial review caught this twice; the prose never absorbed it.
→ Restate every corroboration count as *lineages*, not repos. It changes which "confirmed" facts deserve a hardware check.

**18. No panic-safety / hostile-input analysis.** Every parser reads length-prefixed strings and offsets straight out of device bytes, and the specification notes niimbluelib *throws* on trailing data and that Python impls `struct.unpack` past the end. Nothing states relabel's rule.
→ Doctrine: the packet layer returns `Result`, never `unwrap`/slice-index; add a `cargo-fuzz` target over the frame decoder and the RFID/heartbeat parsers. Cheap, and it is the only place device input reaches memory-unsafe-shaped code.

**19. Error UX and i18n: not researched.** A 53-entry `PrinterErrorCode` table exists with no mapping to what the user should *do*, `0xDB 0x06` is documented as meaning something other than its name, and there is no localisation plan despite rpgp-style Slint conventions.
→ Product work, but needs a pass: map every code to (message, suggested action, is-it-fatal), and decide whether v1 is English-only.

**20. Accessibility of a canvas editor: not mentioned once.** rpgp enables Slint's `accessibility` feature. A drag-and-drop label designer plus a 1-bit bitmap preview is close to the worst case for a screen reader.
→ At minimum decide whether element properties are editable by keyboard/numeric fields (they should be) rather than drag-only.

**21. Font licensing.** `rendering` recommends embedding fonts (correct, given the fontconfig-dlopen escape) and never says which fonts, under what licence, or how the user adds their own. Embedding a font is a redistribution decision in an MIT repo.
→ Pick OFL fonts, record them in `THIRD-PARTY.md`, and design a user-font path (`fontique::Collection::register_fonts`).

**22. Windows dual-mode pairing** (raised in `transport`'s open questions, never answered) is the most likely support ticket: user pairs the printer in Windows Settings as a Classic device, then relabel's GATT path can't reach it.
→ Search bleak/WinRT issues for dual-mode pairing blocking GATT; write the answer into the troubleshooting docs before v1 rather than after.

**23. Concurrent connections.** Vendor material claims the B4 supports 3 iOS / 7 Android simultaneous connections; nothing in the specification says what happens if the official phone app is connected while relabel prints, or whether BLE rejects a second central.
→ Worth one hardware check; at minimum, detect and warn.

---

## Two things that do *not* need more research

Framing, opcode tables and the print-task matrix are done to death and further research there has diminishing returns. The bottleneck is now **hardware access and application design**, not protocol archaeology. If only three items are actioned: **#1 (cols)**, **#6 (threading architecture)**, **#7 (how the user learns label size)**.

Sources consulted: [niimblue types.ts](https://raw.githubusercontent.com/MultiMote/niimblue/main/src/types.ts), [niimbluelib packet_generator.ts](https://raw.githubusercontent.com/MultiMote/niimbluelib/main/src/packets/packet_generator.ts), [niimblue post_process.ts](https://raw.githubusercontent.com/MultiMote/niimblue/main/src/utils/post_process.ts), [Slint discussion #7246](https://github.com/slint-ui/slint/discussions/7246), [Slint discussion #5784](https://github.com/slint-ui/slint/discussions/5784), [slint::spawn_local](https://releases.slint.dev/1.8.0/docs/rust/slint/fn.spawn_local), [NIIMBOT B4 wiki page](https://printers.niim.blue/hardware/niimbot-b4/).