# Antinode — documentation index

`DOC_README.md` is the sole canonical index for this repo (DOC_POLICY §6). Read
this first, then `DOC_POLICY.md`, before starting work.

## What antinode is

An independent desktop client for the HyVibe smart guitar, speaking the
instrument's own JSON-RPC-over-Bluetooth protocol directly. The protocol was
reverse-engineered for interoperability from the vendor's Android app; antinode
reimplements it independently from the recovered facts and interfaces. Woodshed
is the first consumer. Structural template is retinue (sans-io core, thin I/O
shell, receipt-driven validation).

## AI-assistant working principles

- **Prove the instrument.** A protocol fact read from the decompiled app is a
  hypothesis, not a fact, until it is confirmed against the physical guitar in
  the same run as a positive control (a `GetStatus` that returns real values).
  Mark the confidence of every Finding: static-read, fixture-verified, or
  hardware-verified. This is the local addendum's provenance rule; honor it.
- **Expression boundary.** No vendor code, decompiled sources, or firmware
  binaries in this repo. No vendor trademark in a crate name. Findings record
  facts and interfaces and paraphrase behaviour, citing class names for
  provenance only. Say "reverse-engineered", never "clean-room" — the personnel
  split that word denotes is not ours, and DOC_POLICY's addendum explains why
  the distinction is worth keeping straight.
- **Sans-io first.** Protocol logic is pure functions over bytes, replayable
  against fixtures, `no_std`-capable — I/O lives in a separate shell. This is
  what keeps the future-firmware option open without committing to it, and it
  is the retinue discipline the repo is modelled on.
- **Nothing irreversible without sign-off.** Publishing to crates.io, pushing,
  or writing to the guitar's persistent config are Action-tier: they enter
  deliberately, with Mark's go, never as drift from exploration.
- **Follow the workspace method.** New objectives (notably alternative
  firmware) get their own Assess pass and plan before Assemble/Action, rather
  than being folded into an open plan.

## Active documents

| Doc | What's there |
|---|---|
| [`DOC_POLICY.md`](DOC_POLICY.md) | Documentation governance (canonical core + antinode addendum). |
| [`2026-08-27_antinode_founding.md`](2026-08-27_antinode_founding.md) | Founding plan: the decision, crate layout, phases with done-conditions, and the full recovered protocol map (Findings). |

`PROJECT_DESCRIPTION.md` (maintainer-owned, DOC_POLICY §7) is not yet written;
the founding plan carries the product intent until it is.
