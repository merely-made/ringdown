//! Sans-io client protocol for the HyVibe smart guitar.
//!
//! The guitar is driven by JSON-RPC 2.0 carried over a Bluetooth Low Energy
//! GATT service, with a chunking layer beneath it for messages that exceed the
//! negotiated MTU. This crate owns that protocol and nothing else: encoding and
//! decoding, the chunk/reassembly state machine, and the domain model of banks,
//! effects, and typed DSP parameters.
//!
//! It performs no I/O. Bytes go in, bytes and events come out, so the same core
//! serves a desktop Bluetooth shell, a replay harness over captured fixtures,
//! and — should it ever be wanted — an embedded target. Transport lives in a
//! separate crate.
//!
//! # Status
//!
//! Founding stub. The protocol map this crate implements is recorded in
//! `design_docs/2026-08-27_antinode_founding.md`; it was recovered by static
//! analysis and is **unconfirmed against hardware** until the Phase 1 control
//! run. Treat every wire detail as a hypothesis until then.
//!
//! # Interoperability
//!
//! Antinode is an independent implementation, not affiliated with or endorsed
//! by HyVibe. It contains no vendor code: only the interface facts needed to
//! talk to an instrument its owner already has.

#![no_std]
#![forbid(unsafe_code)]
