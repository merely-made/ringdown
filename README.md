# Antinode

An independent desktop client for the HyVibe smart guitar. It speaks the
instrument's own JSON-RPC-over-Bluetooth protocol directly, so the guitar can be
configured — eventually past what the vendor's phone app exposes — from a
computer, with that capability owned rather than rented from an app.

The name is a luthier's: the *antinode* is the point of maximum displacement on
a standing wave, and active vibration control of the guitar's top plate is what
this instrument does.

> **Status (2026-08-27):** just founded. Plan and the full recovered protocol
> map live in [`design_docs/2026-08-27_antinode_founding.md`](design_docs/2026-08-27_antinode_founding.md).
> No code yet. The protocol was mapped by static analysis of the vendor app and
> is unconfirmed against hardware until Phase 1's live proof.

## Design

The protocol was reverse-engineered for interoperability from the vendor's own
Android app. This repo carries no vendor code, no decompiled sources, and no
vendor firmware — only the recovered facts and interfaces, reimplemented
independently. Architecture follows the retinue template — a sans-io protocol
core, a thin Bluetooth I/O shell, and receipt-driven validation.
[Woodshed](https://github.com/merely-made/woodshed) is the first consumer.

Antinode is not affiliated with or endorsed by HyVibe; "HyVibe" is their
trademark, used here only to say what this interoperates with.

Start with [`design_docs/DOC_README.md`](design_docs/DOC_README.md).
