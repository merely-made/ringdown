# Antinode — an independent client for the HyVibe smart guitar

**Date:** 2026-08-27
**Status:** in progress. **Phase 0 landed 2026-08-27** — repo founded, MPL-2.0
(D1), workspace and crate stubbed and building, `antinode` 0.0.1 claimed on
crates.io (D2). No protocol code yet; Phase 1 is next and needs the physical
guitar. Nothing has touched the instrument.

---

## What this is

The HyVibe system turns an acoustic guitar into its own amplifier, multi-effect
processor, looper, and speaker, with the DSP running on hardware inside the
instrument. The only way to configure it is the vendor's iOS/Android app.
**Antinode is an independent desktop client** that speaks the guitar's own
Bluetooth protocol directly, so the instrument can be configured — eventually
past what the phone app exposes — from a computer, and so its capability is
owned rather than rented from an app that could disappear.

The name is a luthier's word: the **antinode** is the point of maximum
displacement on a standing wave. HyVibe's actual mechanism is active vibration
control of the guitar's top plate, and plate antinodes are what a luthier maps
when voicing a top. The name carries the mechanism, not a resemblance to it —
which is the bar this workspace sets for a product-tier name. crates.io was
verified free (API + sparse index) on 2026-08-27.

**Scope of the name.** "HyVibe" is the vendor's trademark and appears in this
repo only as prose describing what antinode interoperates with — never in a
crate name, per the expression boundary in `DOC_POLICY.md`. Antinode is not
affiliated with or endorsed by HyVibe.

---

## The decision: a new repo, on the retinue template

This is a **new repository under `repos/`**, not a subtree of woodshed. The
reasoning:

- **The protocol crate is publish-tier and reusable.** A JSON-RPC-over-BLE
  client for a specific instrument is a library other things can consume; it
  should not be buried inside a practice-toolkit application. This mirrors the
  family's existing posture where `signalman-desktop` roots its own workspace
  outside the protocol repo rather than fusing app and protocol.
- **Woodshed is the first consumer, not the owner.** Woodshed already has the
  metronome, MIDI clock, looper, live-input recording, tuner, and latency
  calibration that this instrument exposes over the wire. It consumes antinode
  as a git dependency for the UI (the same way woodshed consumes
  `genet-host-api` from genet). The two are siblings, not parent and child.
- **The firmware option wants a home of its own.** If the project ever reaches
  alternative firmware (Phase 4, explicitly gated below), that is `no_std`
  embedded work that belongs beside the protocol core, not inside a desktop
  app.

The structural template is **retinue**, which solves a structurally identical
problem for LoRa radios: a sans-io protocol core ("pure functions over bytes,
replayable against fixtures"), a thin I/O shell over it, `no_std` firmware
crates on embassy + esp-hal, a serial-DFU flashing tool, and receipt-driven
validation against a reference implementation. Antinode borrows that shape
directly. In particular, HyVibe's LLT chunking layer is the same class of thing
as retinue's HDLC framing codec in `iface::hdlc` — a sans-io framer with a
replayable fixture suite.

### Intended crate layout (built across Phases 0–2, not pre-built)

```
crates/
  antinode/          Sans-io protocol core. JSON-RPC 2.0 envelope, the LLT
                     chunk/reassembly state machine, and the domain model
                     (Bank, Effect, Parameter, Equalizer, Status, …). No BLE,
                     no async runtime, no_std-capable. Publish-tier; the named
                     crate. Replayable against captured fixtures.
  antinode-ble/      Transport shell: btleplug on desktop (Windows/macOS/Linux).
                     The tokio side of "sans-io core, tokio shell". Plain
                     hyphenated support-crate name per the tier rule; likely
                     publish = false until it has an audience.
apps/
  <spike-cli>/       The consumer that reveals the library's shape (the park.rs
                     / linkboy precedent): connect, GetStatus, ReadConfig, dump.
                     Plain descriptive name, settled in Phase 0. publish = false.
```

Woodshed stays where it is and gains a dependency on `antinode` + `antinode-ble`
plus a view surface — no antinode code lives in the woodshed tree.

---

## Non-goals (v1)

- **No audio over Bluetooth.** The DSP is on the instrument; there is no audio
  stream to carry. Antinode configures the guitar, it does not process its
  sound on the desktop. (Aux-jack audio routing on the guitar is configured via
  RPC, but the samples never touch the computer.)
- **No file-transfer or firmware-upload paths in the client.** The protocol has
  `sendFile` / `.part` chunking machinery; it is out of scope for the client
  and deliberately left alone (§ Findings, "Gaps").
- **No mobile client.** The protocol is identical from iOS, but antinode is a
  desktop project.
- **Firmware is not a v1 goal.** It is Phase 4, gated behind its own
  assessment; see below.

---

## Phases

### Phase 0 — Found the repo and claim the name

**Feature target:** the repo is a real project and the name is secured.

Done-conditions:

- Repo exists at `repos/antinode` with `git init` and the doc scaffold
  (`DOC_POLICY.md`, `DOC_README.md`, this plan). — **met 2026-08-27**
- License chosen (Decision D1) and `LICENSE` added. — **met 2026-08-27**
  (MPL-2.0, byte-identical to retinue's copy)
- Root workspace `Cargo.toml` with the `antinode` core crate stubbed. —
  **met 2026-08-27** (`cargo build` and `cargo package` both clean; the crate
  is `#![no_std]` and `#![forbid(unsafe_code)]` from the first commit, so the
  sans-io posture is enforced by the compiler rather than by intention)
- **crates.io `antinode` 0.0.1 reservation published** (Decision D2). —
  **met 2026-08-27.** The ledger's heddle lesson is explicit: a banked winner
  unclaimed is a winner lost, and `coppice` (banked "clean" on 2026-07-30,
  actually taken since 2025-01) is the fresh reminder that the check and the
  claim are one step.

**Phase 0 is complete.**

### Phase 1 — Protocol core and the live proof (the instrument that measures everything after it)

**Feature target:** antinode connects to the actual guitar, reads its status
and its live effect catalog, and every static claim in Findings is confirmed or
corrected against hardware.

Done-conditions:

- **LLT codec, sans-io, round-trips against captured fixtures.** Chunk a
  message larger than the write length and reassemble it; the fixture set
  includes a payload containing quotes and backslashes, to prove the iterative
  chunk-sizing loop (Findings F5) rather than an arithmetic split. —
  **met 2026-08-27.** `llt` and `handshake` implemented; 21 tests plus a
  doctest, `cargo clippy` and `cargo fmt` clean, no warnings. Covered:
  escaping-heavy payloads, control characters (the six-character worst case),
  multi-byte characters never split across a frame boundary, every frame within
  the write length, contiguous 1-based sequencing, reassembly equalling the
  original, refusal at the un-negotiated 20-byte write length, status-code
  round-trip, and ack-versus-reply demultiplexing. These are property tests
  written against the recovered spec — **not** fixtures captured from a real
  device, which remains a Phase 1 item.
- **JSON-RPC envelope and the 32 typed request/response methods** compile and
  serialize to the wire field names recorded in Findings.
- **`antinode-ble` connects to the guitar over GATT**, negotiates MTU, writes
  to RX (`…4161`), subscribes to notifications on TX (`…4162`).
- **The spike CLI issues `GetStatus` and prints** `device`, `cpuID`,
  `battLeft`, `versionESP`, and `versionSTM` from the real instrument. This is
  the positive control: it promotes the whole static map from static-read to
  hardware-verified in one shot, per the repo's provenance rule.
- **`ReadConfig` returns the live effect catalog**, and it is persisted as the
  authoritative fixture. This closes the "effect catalog lives in the vendor's
  Firebase, not the APK" gap (Findings, Gaps) by reading it from the guitar
  instead.
- **The two hardware unknowns are answered** and recorded in Findings: does the
  device require BLE bonding, and how does it advertise (name, service UUID in
  the advertisement)?
- **The connect sequence is confirmed against hardware** (F9), including that
  the version banner read (F10) returns what the static read predicts, and
  which of the two banner forms this guitar uses.
- **How an over-MTU response arrives is established** (F11's open question),
  and inbound handling is written to match what was captured rather than to a
  guess.

### Phase 2 — Full configuration parity

**Feature target:** everything the phone app can do, antinode can do, from the
desktop.

Done-conditions:

- **Every method's `params` shape is bound** — the ~30 request classes mapped
  from the recovered parameter vocabulary (Findings F4). Mechanical, not hard.
- **The write commands are implemented and exercised:** bank CRUD
  (Add/Remove/Move/SwitchBank, SetBankName, SetGainBank), effect CRUD
  (Add/Update/Remove/MoveEffect), EQ (SetEQGain, SetEQBandGain), aux routing
  (AuxIn/AuxOut and their dry/wet), metronome (Start/Stop/Update), recording
  (Start/Stop), SustainKiller, and SetController.
- **A round-trip receipt per command family:** `ReadBank` → mutate → write →
  `ReadBank` reflects the change, captured as a replayable receipt in the
  retinue discipline.
- **Woodshed consumes antinode:** git dependency wired, one view surface that
  connects and switches banks, proving the sibling-consumer topology end to
  end.

### Phase 3 — Beyond the app

**Feature target:** the things the phone app cannot do — the reason to own the
protocol rather than rent it.

Candidate scope (Decision D3 sets the actual target):

- Preset (bank) management as real files on the desktop: export, import,
  version, diff, share.
- Controller remapping via `SetController` beyond the app's partial exposure
  (`Control.source` binds a physical knob/pedal to a parameter with its own
  min/max scaling).
- Parameter automation, and driving the guitar's metronome/looper transport
  from woodshed's own.

Done-conditions: **deferred**, set once Phase 1 reveals what the DSP actually
implements.

### Phase 4 — Alternative firmware (SEPARATE OBJECTIVE — its own assessment required)

This phase is recorded, not planned. Per the workspace method, open-source
firmware is a **new objective** and gets its own Assess pass before any Assemble
or Action; it is not folded into this client project. What Phase 1 buys is the
instrumentation that makes that assessment possible — `GetStatus.cpuID` names
the exact silicon, `ReadConfig` enumerates what the current DSP does.

Known shape, and the honest blockers, so the future assessment starts informed:

- **Two processors.** An ESP32 handles connectivity (it owns the RPC layer);
  an **STM32 runs the audio DSP** (Findings F6). Mark's existing embedded work
  is esp-hal on ESP32-S3 (retinue) — that transfers to the *connectivity* half
  but not the audio half. embassy supports STM32 via `embassy-stm32`, but no
  existing driver applies.
- **Signing is unknown, not absent.** No crypto appears anywhere in the
  vendor's *app* protocol or transport code (Findings F7). That is the app's
  silence; firmware-signature verification would live on the device, where the
  app cannot see it. By the workspace rule, an absence is evidence only with a
  positive control in the same run, and there is none. A failed flash likely
  means opening the guitar to reach STM32 SWD pads.
- **Firmware ships as `.hef` over USB** with a user-accessible bootloader (hold
  center + tap power), per vendor docs — a documented, non-exotic entry point,
  format not yet mapped.
- **The FX bridge exists.** FAUST compiles to embedded C++ for STM32
  (Electrosmith's Daisy is an STM32H7 doing exactly this), and
  [Guitarix](https://guitarix.org/) (25+ amp/effect modules) was prototyped in
  FAUST. **Licensing caveat:** Guitarix is GPLv2, which would infect derived
  firmware and sits crosswise to this workspace's usual permissive posture; the
  FAUST standard libraries are more permissively licensed and are the better
  starting point.

Entry condition: a completed Phase 4 assessment doc with its own done-conditions
and Mark's sign-off. Nothing in Phases 0–3 depends on this phase.

---

## Findings — the recovered protocol (2026-08-27)

Source: static analysis (jadx 1.5.6) of the vendor Android app, package
`audio.hyvibe.app` v1.1.2 (versionCode 4022), APK SHA-256
`77fec6aa…2dc8c303`. **Confidence: static-read** for everything below until
Phase 1's positive control promotes it to hardware-verified. Class names are
cited for provenance only; no vendor source is reproduced here (expression
boundary). An interactive rendering of this map was produced during the
assessment; this section is the durable authoritative record.

The app's own code splits three ways: `com.xsquad.*` is a generic BLE
transport, `com.hyvibe.*` is the device protocol and domain model (the portable
part, no Android deps in the protocol path), and `net.weeteam.hyvibe.*` is
Android UI. Antinode reimplements the middle layer.

**F1 — BLE surface** (`com.hyvibe.adapters.BLEConstantsKt`):

| Role | UUID | Direction |
|---|---|---|
| Guitar service | `eb65b6c6-fec3-4ed1-a6fc-9eff755a4160` | — |
| RX — request | `eb65b6c6-fec3-4ed1-a6fc-9eff755a4161` | client writes → |
| TX — response | `eb65b6c6-fec3-4ed1-a6fc-9eff755a4162` | ← notifications |

RX is the write characteristic, TX is the notify characteristic
(`BLEConnectableDevice`). There is a second, app-side service
(`d6db7b7f-…`) with a Nordic-UART-style characteristic (`6E400003-…`); it is
not the guitar-control path and is out of scope until Phase 1 says otherwise.

**F2 — MTU / write length.** On connect the client requests MTU 517; usable
write length is `mtu − 3`, i.e. **514 bytes** on success, falling back to **20**
if the peer refuses (`BLEConnectableDevice`: `requestMtu(517)`,
`maxWriteLength = mtu − 3`, default 20). This value is the input to the
chunking decision in F5, so it is tracked at runtime, never assumed.

**F3 — Transport stack, four layers.**
1. BLE GATT (F1) — one service, write + notify.
2. **LLT framing** — chunks any RPC message larger than the write length into
   acknowledged, sequenced pieces; bypassed when a message already fits.
3. **JSON-RPC 2.0** — standard envelope, wire fields `jsonrpc`, `id`, `method`,
   `params`, `result` (`com.hyvibe.models.RPC.*`). 10-second default request
   timeout (`LLTManager`).
4. Domain model (F4).

LLT frame fields (`LLTRequest`, `@SerialName`): `oid` (object id — mirrors the
JSON-RPC `id`), `mid` (chunk sequence, 1-based), `d` (escaped payload slice),
`s` (optional total unescaped length), `n` (optional filename, file transfer
only). Frames are newline-terminated; payload is backslash/quote-escaped. LLT
status codes (`LLTCode`): `0 LLT_ABORT, 1 LLT_OK, 2 LLT_DONE, 3 LLT_TIMEOUT,
4 LLT_OVERLOAD, 5 LLT_MALFORMED, 6 LLT_MISSING_CHUNK, 7 LLT_WRONG_MID,
8 LLT_WRONG_OID`. The device ACKs each chunk with a code before the next is
sent; `LLT_OK` means continue.

**F4 — The 32 RPC methods** (`RPCRequestType`, wire names exact):

- System: `GetStatus`, `GetVersion`, `SetDate`, `Calibrate`
- Config: `ReadConfig`, `SaveConfig`, `SetConfig`
- Banks/presets: `ReadBank`, `SwitchBank`, `AddBank`, `RemoveBank`, `MoveBank`,
  `SetBankName`, `SetGainBank`
- Effects: `AddEffect`, `UpdateEffect`, `RemoveEffect`, `MoveEffect`,
  `SetController`
- EQ: `SetEQGain`, `SetEQBandGain`
- Aux routing: `AuxIn`, `AuxInDryWet`, `AuxOut`, `AuxOutDryWet`
- Metronome: `StartMetronome`, `StopMetronome`, `UpdateMetronome`
- Recording: `StartRecording`, `StopRecording`
- Other: `SustainKiller`, `GetFileInfo`

Recovered param vocabulary (from `ConnectableDeviceCommunicator`) includes
`bankNumber`, `effectNumber`, `bandNumber`, `gainValue`, `knobValue`, `bpm`,
`num`/`den`/`nbBars`, `parameter`, `options`, `source`, and date fields — the
per-method binding is the mechanical Phase 2 pass.

**F5 — LLT chunk sizing is iterative, not arithmetic.** The app shrinks each
chunk in a loop until the fully serialized frame fits the write length, because
escaping changes length unpredictably (`LLTManager.sendMessage`). A client that
computes slice size once from an average overflows the MTU on any payload
containing quotes or backslashes. The Phase 1 fixture set must include such a
payload.

**F6 — Two processors.** `Status` (`com.hyvibe.models.Status`) reports
`versionESP` and `versionSTM` separately, plus `device`, `cpuID`, `battLeft`,
`freeSpaceGb`, `freeSpacePct`. Interpretation: ESP32 for connectivity, STM32 for
the audio DSP; the separate firmware versions and the file-transfer machinery
are consistent with two independently-flashed images. Hardware-verify in
Phase 1.

**F7 — No authentication in the protocol.** No pairing token, challenge, or
crypto anywhere in `com.hyvibe.*` or `com.xsquad.*`. The Firebase account the
app requires is for preset sync and effect-definition download, not for talking
to the guitar. A client that can reach the GATT service can issue any command.
(This is the app-side view; whether the device gates anything is a Phase 1
question.)

**F8 — DSP domain model.**
- `Effect` = `{ type, preset, bypass, params[] }`
- `Parameter` = `{ key, value: float, control }`
- `ParamDefinition` = `{ key, min, max, defaultValue, precision, unit,
  scale_func, scale_fact, control }` — the full typed definition of a knob,
  including the named function mapping control position to DSP value.
- `Control` = `{ min, max, source }` — binds a physical control to a parameter.
- `EffectDefinition` = `{ name, version, file, guitar_min_version, params[] }`.
- `Bank` = an ordered chain of effects (a preset).

**F9 — The connection sequence, in order.** From `BLEConnectableDevice`'s GATT
callbacks, `onServicesDiscovered` onward:

1. Discover the guitar service (F1).
2. Set the **request** characteristic's write type to Android's `2`
   (`WRITE_TYPE_DEFAULT`) — i.e. **write with response**, not
   write-without-response. Consistent with a device that acknowledges every
   frame; a client must not "optimise" this to no-response.
3. Enable notifications on the **response** characteristic.
4. **Read** the response characteristic once — this returns the version banner
   (F10), not a JSON message.
5. `requestMtu(517)`.
6. Signal ready; RPC may begin.

**F10 — A version banner precedes RPC, and it is not JSON.** The connect-time
read of the response characteristic returns one of two plain-text forms:

- `S<stm>_E<esp>\n` — e.g. `S1.2.3_E2.7.0`; both processors.
- `@version <esp>\n` — older firmware, ESP only; the vendor's client then
  **assumes** an STM version of `1.2.2` rather than reading one.

This is distinct from, and earlier than, the `GetVersion` RPC method. Antinode
parses both forms and flags whether the STM version was reported or assumed
(`handshake::Banner::stm_was_implied`), because an inherited assumption should
not be indistinguishable from a device-reported fact.

**F11 — Inbound has no reassembly, and the response characteristic is
multiplexed.** `receiveMessage` UTF-8-decodes each notification and fans it
straight out to listeners with **no buffering and no reassembly**. Consequences:

- Every notification is expected to be a complete message on its own.
- The same characteristic carries *both* LLT acknowledgements and complete
  JSON-RPC replies, so a client demultiplexes by shape — try to parse an ack,
  and treat a non-match as a JSON-RPC reply rather than as an error. Antinode
  does this in `llt::Ack::parse`, which returns `Option` for exactly this
  reason.
- **Open question for Phase 1:** how a response larger than the MTU arrives,
  given the vendor's client cannot reassemble one. Either large replies are
  chunked by a mechanism not visible in the app, or `ReadConfig` returns
  something smaller than expected. Do not build inbound reassembly on
  speculation — capture a real `ReadConfig` first.

**Gaps (need the physical guitar, not more decompiling):**
- The effect *catalog* (which effects exist, with their `ParamDefinition`s)
  lives in the vendor's Firebase (`hyvibe_factory`), not the APK. Recovered
  instead from the instrument via `ReadConfig` in Phase 1.
- BLE bonding requirement and advertisement format — Phase 1.
- File-transfer / firmware-upload wire path (`sendFile`, `.part`) — unmapped,
  and deliberately out of scope for the client.

**Provenance caveat.** The APK's signing certificate carries the generic
placeholder subject `CN=Android, O=Google Inc.` rather than a HyVibe developer
key, so the signature does not independently attest authorship. Nothing in the
decompiled code looks tampered with and the 2019 validity start fits the app's
era, but this build was only ever read, never run. The protocol is a hypothesis
until Phase 1's control run.

---

## Findings — the vendor's licensing posture (2026-08-27)

Checked because the project's footing depends on it. **Not legal advice**; this
records what was actually found, so a future decision starts from facts rather
than assumption. Anything commercial warrants a real lawyer.

**L1 — No EULA reaches the user.** The app's `terms_of_use` string resource is a
bare twelve-character *label* ("terms of use"), not a document; registration
shows an "I agree with the terms of use" checkbox with **no terms body and no
URL bundled anywhere in the APK**. The only http(s) URLs in app resources are
Google/Firebase policy links, the Firebase instance, and a Play Store link.

**L2 — No anti-reverse-engineering clause exists to breach.** Zero occurrences
of "reverse engineer", "decompile", or "disassemble" across the app's entire
string resources.

**L3 — The bundled privacy policy is generator boilerplate with an unfilled
placeholder.** Effective 2020-07-27; it refers to "our Terms and Conditions,
which is accessible at HyVibe" — the site name was never substituted for a
link. `hyvibeguitar.com/terms-and-conditions/` returns **404**.

**L4 — The only site-wide legal document is a Terms of *Sale*.** It governs
e-commerce (sales, shipping, returns), not software licensing. Its IP clause
(Article 13) reserves rights in **website** elements only — not the purchased
product, its software, or its firmware.

**Consequence for Phases 0–3 (the client).** The sharpest constraint on
interoperability work — a contractual anti-RE clause in an accepted EULA — is
simply **absent** here. What remains is copyright, where intermediate copying to
extract interface facts for interoperability is long-settled fair use in the US
(*Sega v. Accolade*, 9th Cir. 1992; *Sony v. Connectix*, 9th Cir. 2000), and
where interfaces and functional facts are not protectable expression in the
first place (17 U.S.C. §102(b); *Google v. Oracle*, 2021). DMCA §1201
anti-circumvention does not engage: a standard APK is an ordinary ZIP with no
technological protection measure to circumvent, and §1201(f) exempts
interoperability reverse engineering regardless. Trade-secret law treats reverse
engineering as a proper means of acquisition absent a confidential relationship;
there is none. The operative discipline is therefore the **expression boundary**
in `DOC_POLICY.md`, which this repo already keeps.

**Consequence for Phase 4 (firmware) — materially different, and this is the
real reason the phase stays gated.** Replacing firmware is not the same legal
act as talking to a documented-by-inspection radio protocol:

- If the device *does* verify firmware signatures, that verification is
  plausibly a technological protection measure, and defeating it engages DMCA
  §1201 in a way the client work never does. §1201(f) may or may not cover it;
  the exemption is narrower than it is usually assumed to be. Recall F7: signing
  is **unknown, not absent**.
- Vendor firmware images are the vendor's copyrighted work: they may be analysed
  but never redistributed, patched-and-redistributed, or vendored into this repo.
- Warranty consequences are real and separate from copyright.

None of this blocks Phase 4; it defines what its assessment must actually
answer, alongside the technical blockers already recorded.

**Provenance-of-the-APK caveat (repeat of the teardown note, relevant here).**
The analysed build came from a third-party mirror (APKPure), not Google Play,
and its signing certificate carries a generic placeholder subject rather than a
HyVibe developer key — so authorship is not independently attested. Phase 1's
hardware control run is what makes this moot: whatever the real guitar answers
is the truth, whatever the APK said.

## Decisions (Mark's — pending)

- **D1 — License. DECIDED 2026-08-27: `MPL-2.0`**, matching the retinue family
  rather than woodshed's `MIT OR Apache-2.0`. `LICENSE` is byte-identical to
  retinue's copy. Two consequences worth recording, because they were not the
  reason for the choice but they fall out of it well:
  - **File-level copyleft is the right shape for a protocol crate.**
    Improvements to antinode's own files stay open, while the crate can still
    be linked into differently-licensed applications — including a proprietary
    one, which a hard copyleft would have foreclosed.
  - **It keeps the Phase 4 FX path open.** MPL-2.0 §3.3 permits distributing a
    Larger Work under a **Secondary License** (GPL 2.0+, LGPL 2.1+, AGPL 3.0+)
    so long as the covered files are not marked "Incompatible With Secondary
    Licenses" — and ours are not. So the GPLv2 tension flagged against Guitarix
    (Phase 4) does **not** bite antinode: MPL-2.0 code may be combined into a
    GPL work. Had this gone MIT/Apache the answer would also have been fine;
    the point is that MPL-2.0 costs nothing here and is not the trap a
    copyleft-adjacent license might look like at first glance.
- **D2 — Claim the crates.io name. DONE 2026-08-27.** `antinode` 0.0.1
  published to crates.io (MPL-2.0), confirmed live via the registry API
  independently of cargo's own success message. The name is claimed with a real
  publish rather than an intention, per the heddle lesson. Note for the ledger:
  the crate is a **stub** — a documented `no_std` lib with no protocol code —
  which is the point. The reservation exists to hold the name while Phase 1 is
  built, exactly as `mora` and the signalman/postilion/linkboy trio were
  founded.
- **D3 — Phase 3 scope.** Which "beyond the app" capability leads. Set after
  Phase 1.
- **D4 — Is Phase 4 (firmware) ever entered?** Open. Requires its own
  assessment doc first regardless.

## Open questions (answered by Phase 1, not by debate)

- Does the guitar require BLE bonding / pairing?
- Exact STM32 and ESP part numbers (`cpuID`, version strings).
- The real effect catalog and every parameter's range/unit/scaling.

---

## Progress

- **2026-08-27** — Repo founded: `git init`, `.gitignore`, `DOC_POLICY.md`
  (canonical core + antinode addendum), `DOC_README.md`, this plan. Protocol
  map recorded in Findings from the v1.1.2 APK teardown.
- **2026-08-27** — Vendor licensing posture checked and recorded (Findings
  L1–L4): no EULA reaches the user, no anti-reverse-engineering clause exists,
  and the only site-wide legal document is a Terms of Sale governing
  e-commerce. Consequences split by phase; Phase 4's are the ones that bite.
- **2026-08-27** — Terminology corrected across all four docs: the work is
  **reverse engineering for interoperability**, not clean-room. The earlier
  "clean-room" wording was an overclaim — that term denotes a personnel split
  (one party reads, a separate party implements) which this project does not
  have. `DOC_POLICY.md`'s addendum now names the rule "expression boundary" and
  explains the distinction, and records how a genuine split could be arranged
  later from the Findings spec if one is ever wanted.
- **2026-08-27** — D1 decided (MPL-2.0). `LICENSE` installed, workspace and
  `crates/antinode` stub created; `cargo build` and `cargo package` clean.
- **2026-08-27 — PHASE 0 LANDED.** Initial commit on `main` (branch renamed
  from git's `master` default to match the family's four other repos).
  `antinode` 0.0.1 published to crates.io under MPL-2.0 and confirmed live via
  the registry API.
- **2026-08-27 — Phase 1 transport layer landed** (the half that needs no
  hardware). `llt` (framing, chunking, ack parsing) and `handshake` (the
  connect-time version banner) implemented sans-io on `alloc`; 21 tests and a
  doctest green, clippy and fmt clean, zero warnings.
  - Three findings were recovered while grounding the code and are now recorded:
    **F9** the ordered connect sequence including write-with-response on the
    request characteristic, **F10** the pre-RPC plain-text version banner in two
    forms, and **F11** that inbound has no reassembly and the response
    characteristic multiplexes acks with replies.
  - F11 leaves a real open question — how an over-MTU response arrives at all —
    which is deliberately **not** solved in code. Writing speculative inbound
    reassembly would violate the repo's provenance rule; it waits for a captured
    `ReadConfig`.
  - Next: `rpc` (the JSON-RPC envelope and the 32 typed methods) is also
    desk-doable. The phase does not close until `GetStatus` answers from the
    physical guitar.
