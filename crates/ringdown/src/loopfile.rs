//! Loop recordings: the WAV header the instrument writes, and the tempo in it.
//!
//! A loop is an ordinary RIFF/WAVE file with one vendor extension: a `JUNK`
//! chunk labelled `"HyVibe loop file"` carrying six 32-bit little-endian
//! values. `JUNK` is the RIFF chunk type readers are required to skip, so the
//! audio plays anywhere while the metadata is invisible to everything that does
//! not look for it.
//!
//! That metadata is worth recovering because it makes a loop importable *in
//! time*: a recording that arrives knowing its own tempo can be dropped onto a
//! grid instead of being stretched by ear.
//!
//! # What the six values are
//!
//! From `/Loops/loop0031.wav` on the reference instrument they are
//! `1, 200, 7, 8, 4, 0`. Two of them are established by arithmetic against the
//! audio itself, and the rest are not, so this module names only the first two
//! and leaves the others as what they are.
//!
//! The audio is 741,376 bytes of 16-bit mono at 44,100 Hz — **370,688 samples,
//! 8.4056 s**. Against that:
//!
//! - **`200` is the tempo.** 7 × 4 = 28 beats at 200 BPM is 8.400 s, which
//!   matches to 5.6 ms. No other product of these values fits: 7 × 8 = 56
//!   beats would be 16.8 s and 8 × 4 = 32 would be 9.6 s, both far outside the
//!   file. So `8` is not a length field, and 28 is the loop's length in beats.
//! - **The 5.6 ms is block rounding, not error.** 370,688 is exactly
//!   181 × 2048, and 2048 is the largest power of two that divides it — 4096
//!   does not. The tempo-exact length of 370,440 samples is 180.88 blocks, and
//!   rounding *up* to 181 gives the file size precisely. The recorder captures
//!   whole 2048-sample DSP blocks.
//!
//! **Which of `7` and `4` is bars and which is beats-per-bar is not
//! determined**, because only their product appears in the audio; 7 bars of 4
//! and 4 bars of 7 are the same 28 beats. They are therefore
//! [`LoopMeta::length_first`] and [`LoopMeta::length_second`] here rather than
//! names that would assert more than was measured. The one previous guess at a
//! field's meaning on this protocol — reading `den` as a time-signature
//! denominator — was wrong, and cost a write to someone's instrument.
//!
//! `8` is unexplained. It is suggestive that `ReadMetronome` on the same
//! instrument returns `den: 8` while its metronome is demonstrably in 5/4, so
//! whatever `den` means it is not a denominator, and the same 8 appearing here
//! is more likely the same field than a coincidence. `1` is almost certainly a
//! format version and `0` is almost certainly spare, but neither is tested.
//!
//! # Settling the rest cheaply
//!
//! Everything above comes from a single file, and one more file would separate
//! the length fields — a loop whose bar count differs from its meter breaks the
//! symmetry immediately.
//!
//! The good news is that this costs almost nothing. `DumpFile` takes an offset
//! and a size, so a loop's metadata is **one round trip of
//! [`HEADER_PREFIX`] bytes**, not the ten-minute transfer a whole loop needs.
//! Indexing an entire library is seconds of work, which makes browsing by
//! tempo practical even though fetching by audio is not.

use core::fmt;

/// Bytes to read from the start of a loop to cover everything here.
///
/// Every loop seen lays its header out identically — `RIFF`/`WAVE`, the 40-byte
/// `JUNK`, a 16-byte `fmt `, then `data` — which puts the first audio sample at
/// exactly this offset. [`parse`] walks the chunks rather than assuming that,
/// so a file that differs is reported as [`LoopError::Truncated`] instead of
/// being misread.
pub const HEADER_PREFIX: usize = 92;

/// The label the vendor writes at the start of its `JUNK` chunk.
pub const JUNK_LABEL: &[u8] = b"HyVibe loop file";

/// Count of 32-bit values following [`JUNK_LABEL`].
pub const META_VALUES: usize = 6;

/// The DSP block the recorder rounds a loop's length up to, in samples.
///
/// Derived, not assumed: see the module documentation.
pub const BLOCK_SAMPLES: u32 = 2048;

/// The vendor's metadata, as six 32-bit values.
///
/// Only [`version`](Self::version) and [`tempo_bpm`](Self::tempo_bpm) are named
/// for what they mean, because only those two are established. The rest keep
/// positional names on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopMeta {
    /// Format version. `1` in every file seen; untested beyond that.
    pub version: u32,
    /// Tempo in beats per minute. Confirmed against the audio's own length.
    pub tempo_bpm: u32,
    /// First length field. `7` in the reference file.
    ///
    /// This and [`length_second`](Self::length_second) multiply to the loop's
    /// length in beats — see [`beats`](Self::beats). Which one counts bars is
    /// not known.
    pub length_first: u32,
    /// The value that is not a length. `8` in the reference file, and `8` is
    /// also what `ReadMetronome` reports as `den` on an instrument set to 5/4.
    pub den: u32,
    /// Second length field. `4` in the reference file.
    pub length_second: u32,
    /// Trailing value. `0` in the reference file; purpose unknown.
    pub trailing: u32,
}

impl LoopMeta {
    /// The loop's length in beats.
    ///
    /// The product is measured; its factorisation is not. A caller wanting to
    /// draw bar lines needs to know which field is which, and this module
    /// cannot yet say.
    pub fn beats(&self) -> u32 {
        self.length_first.saturating_mul(self.length_second)
    }

    /// How long [`beats`](Self::beats) lasts at [`tempo_bpm`](Self::tempo_bpm),
    /// in samples at `sample_rate`, before block rounding.
    ///
    /// `None` when the tempo is zero, which no real file should carry but a
    /// corrupt one might.
    pub fn nominal_samples(&self, sample_rate: u32) -> Option<u64> {
        if self.tempo_bpm == 0 {
            return None;
        }
        Some(u64::from(self.beats()) * u64::from(sample_rate) * 60 / u64::from(self.tempo_bpm))
    }

    /// [`nominal_samples`](Self::nominal_samples) rounded up to a whole
    /// [`BLOCK_SAMPLES`] block, which is what the recorder actually writes.
    pub fn expected_samples(&self, sample_rate: u32) -> Option<u64> {
        let nominal = self.nominal_samples(sample_rate)?;
        let block = u64::from(BLOCK_SAMPLES);
        Some(nominal.div_ceil(block) * block)
    }
}

/// The `fmt ` chunk's description of the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// WAVE format tag; `1` is uncompressed PCM.
    pub format_tag: u16,
    /// Channel count.
    pub channels: u16,
    /// Samples per second.
    pub sample_rate: u32,
    /// Bits in one sample of one channel.
    pub bits_per_sample: u16,
}

impl Format {
    /// Bytes occupied by one sample across all channels.
    pub fn frame_bytes(&self) -> u32 {
        u32::from(self.channels) * u32::from(self.bits_per_sample).div_ceil(8)
    }
}

/// A parsed loop header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopHeader {
    /// What the `fmt ` chunk declares.
    pub format: Format,
    /// Length of the `data` chunk in bytes.
    pub audio_bytes: u32,
    /// Offset of the first audio byte within the file.
    pub audio_offset: usize,
    /// Total file length implied by the `RIFF` header.
    pub file_bytes: u64,
    /// The vendor's metadata, when the `JUNK` chunk carries its label.
    ///
    /// `None` for a WAV that is not one of these — a file copied in over USB,
    /// say — which is a fact worth surfacing rather than defaulting away.
    pub meta: Option<LoopMeta>,
}

impl LoopHeader {
    /// Audio length in whole samples per channel.
    pub fn samples(&self) -> u32 {
        let frame = self.format.frame_bytes();
        if frame == 0 {
            return 0;
        }
        self.audio_bytes / frame
    }

    /// Audio duration in seconds.
    pub fn seconds(&self) -> f64 {
        if self.format.sample_rate == 0 {
            return 0.0;
        }
        f64::from(self.samples()) / f64::from(self.format.sample_rate)
    }

    /// Whether the audio is the length the metadata's tempo implies.
    ///
    /// The check that established what these fields mean, kept executable: a
    /// file whose header disagrees with its own audio means the reading here is
    /// wrong for that file, and it is better to hear that from the parser than
    /// to import it silently at the wrong tempo.
    ///
    /// `None` when there is no metadata to check against.
    pub fn tempo_agrees_with_audio(&self) -> Option<bool> {
        let expected = self.meta?.expected_samples(self.format.sample_rate)?;
        Some(expected == u64::from(self.samples()))
    }
}

/// Why a loop header could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopError {
    /// The bytes do not begin `RIFF....WAVE`.
    NotRiffWave,
    /// The header ran out before a required chunk was found.
    ///
    /// Carries how many bytes were supplied, since the usual cause is asking
    /// the instrument for too small a prefix rather than a damaged file.
    Truncated {
        /// Bytes that were available.
        got: usize,
    },
    /// No `fmt ` chunk appeared before the end of the supplied bytes.
    NoFormat,
    /// No `data` chunk appeared before the end of the supplied bytes.
    NoData,
}

impl fmt::Display for LoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopError::NotRiffWave => write!(f, "not a RIFF/WAVE file"),
            LoopError::Truncated { got } => write!(
                f,
                "header incomplete in {got} bytes; read at least {HEADER_PREFIX}"
            ),
            LoopError::NoFormat => write!(f, "no fmt chunk"),
            LoopError::NoData => write!(f, "no data chunk"),
        }
    }
}

impl core::error::Error for LoopError {}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// Read a loop's header from the first bytes of the file.
///
/// Needs only a prefix — [`HEADER_PREFIX`] covers every loop seen — which is
/// what makes indexing a library over Bluetooth affordable.
pub fn parse(data: &[u8]) -> Result<LoopHeader, LoopError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        if data.len() < 12 {
            return Err(LoopError::Truncated { got: data.len() });
        }
        return Err(LoopError::NotRiffWave);
    }
    let file_bytes =
        u64::from(u32_at(data, 4).ok_or(LoopError::Truncated { got: data.len() })?) + 8;

    let mut format = None;
    let mut audio = None;
    let mut meta = None;

    // Walk the chunk list rather than assuming the vendor's exact layout. A
    // chunk's payload is padded to an even length, which is easy to forget and
    // shifts every later chunk by one when it happens.
    let mut at = 12usize;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let size = u32_at(data, at + 4).ok_or(LoopError::Truncated { got: data.len() })? as usize;
        let body = at + 8;

        match id {
            b"fmt " => {
                if body + 16 > data.len() {
                    return Err(LoopError::Truncated { got: data.len() });
                }
                format = Some(Format {
                    format_tag: u16_at(data, body).unwrap(),
                    channels: u16_at(data, body + 2).unwrap(),
                    sample_rate: u32_at(data, body + 4).unwrap(),
                    bits_per_sample: u16_at(data, body + 14).unwrap(),
                });
            }
            b"data" => {
                // The payload is the audio, and is not expected to be present.
                audio = Some((size as u32, body));
                break;
            }
            b"JUNK" => {
                meta = parse_meta(data.get(body..body + size).unwrap_or(&[]));
            }
            _ => {}
        }

        at = body + size + (size & 1);
    }

    let format = format.ok_or(if data.len() < HEADER_PREFIX {
        LoopError::Truncated { got: data.len() }
    } else {
        LoopError::NoFormat
    })?;
    let (audio_bytes, audio_offset) = audio.ok_or(if data.len() < HEADER_PREFIX {
        LoopError::Truncated { got: data.len() }
    } else {
        LoopError::NoData
    })?;

    Ok(LoopHeader {
        format,
        audio_bytes,
        audio_offset,
        file_bytes,
        meta,
    })
}

/// Read the vendor's six values out of a `JUNK` payload, if it is theirs.
fn parse_meta(body: &[u8]) -> Option<LoopMeta> {
    if body.len() < JUNK_LABEL.len() + META_VALUES * 4 {
        return None;
    }
    if &body[..JUNK_LABEL.len()] != JUNK_LABEL {
        return None;
    }
    let v = |i: usize| u32_at(body, JUNK_LABEL.len() + i * 4);
    Some(LoopMeta {
        version: v(0)?,
        tempo_bpm: v(1)?,
        length_first: v(2)?,
        den: v(3)?,
        length_second: v(4)?,
        trailing: v(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 92 bytes of `/Loops/loop0031.wav`, read off the reference
    /// instrument on 2026-08-27. Everything this module claims is claimed about
    /// these bytes.
    const LOOP0031: [u8; 92] = [
        0x52, 0x49, 0x46, 0x46, 0x54, 0x50, 0x0b, 0x00, // RIFF, size
        0x57, 0x41, 0x56, 0x45, // WAVE
        0x4a, 0x55, 0x4e, 0x4b, 0x28, 0x00, 0x00, 0x00, // JUNK, 40
        0x48, 0x79, 0x56, 0x69, 0x62, 0x65, 0x20, 0x6c, // "HyVibe l"
        0x6f, 0x6f, 0x70, 0x20, 0x66, 0x69, 0x6c, 0x65, // "oop file"
        0x01, 0x00, 0x00, 0x00, // 1
        0xc8, 0x00, 0x00, 0x00, // 200
        0x07, 0x00, 0x00, 0x00, // 7
        0x08, 0x00, 0x00, 0x00, // 8
        0x04, 0x00, 0x00, 0x00, // 4
        0x00, 0x00, 0x00, 0x00, // 0
        0x66, 0x6d, 0x74, 0x20, 0x10, 0x00, 0x00, 0x00, // fmt , 16
        0x01, 0x00, // PCM
        0x01, 0x00, // mono
        0x44, 0xac, 0x00, 0x00, // 44100
        0x88, 0x58, 0x01, 0x00, // byte rate
        0x02, 0x00, // block align
        0x10, 0x00, // 16 bits
        0x64, 0x61, 0x74, 0x61, 0x00, 0x50, 0x0b, 0x00, // data, 741376
    ];

    fn reference() -> LoopHeader {
        parse(&LOOP0031).expect("the reference header must parse")
    }

    #[test]
    fn the_reference_header_reads_as_the_file_it_came_from() {
        let h = reference();
        assert_eq!(h.format.format_tag, 1);
        assert_eq!(h.format.channels, 1);
        assert_eq!(h.format.sample_rate, 44_100);
        assert_eq!(h.format.bits_per_sample, 16);
        assert_eq!(h.audio_bytes, 741_376);
        assert_eq!(h.audio_offset, HEADER_PREFIX);
        assert_eq!(h.samples(), 370_688);
    }

    /// `GetFileInfo` reported 741,468 bytes for this path. The RIFF header says
    /// the same thing by a completely different route, which is what made the
    /// file transfer trustworthy in the first place.
    #[test]
    fn the_riff_size_agrees_with_what_the_instrument_reported() {
        assert_eq!(reference().file_bytes, 741_468);
    }

    #[test]
    fn the_vendor_metadata_is_the_six_values_observed() {
        let m = reference().meta.expect("the JUNK chunk is labelled");
        assert_eq!(m.version, 1);
        assert_eq!(m.tempo_bpm, 200);
        assert_eq!(m.length_first, 7);
        assert_eq!(m.den, 8);
        assert_eq!(m.length_second, 4);
        assert_eq!(m.trailing, 0);
        assert_eq!(m.beats(), 28);
    }

    /// The whole identification, in one assertion: 28 beats at 200 BPM, rounded
    /// up to a whole DSP block, is the audio length exactly.
    #[test]
    fn the_tempo_predicts_the_audio_length() {
        let h = reference();
        assert_eq!(h.tempo_agrees_with_audio(), Some(true));
        assert_eq!(h.meta.unwrap().nominal_samples(44_100), Some(370_440));
        assert_eq!(h.meta.unwrap().expected_samples(44_100), Some(370_688));
    }

    /// Block rounding is the explanation for the 248-sample excess, so it has
    /// to be doing real work — if the nominal length were already aligned the
    /// test above would pass whether or not this module understood why.
    #[test]
    fn the_block_rounding_is_what_closes_the_gap() {
        let m = reference().meta.unwrap();
        let nominal = m.nominal_samples(44_100).unwrap();
        assert_ne!(nominal % u64::from(BLOCK_SAMPLES), 0, "nothing to round");
        assert_eq!(m.expected_samples(44_100).unwrap() - nominal, 248);
    }

    /// 2048 is the block size because it is the largest power of two that
    /// divides the audio. Pinning this stops a later "tidy-up" from changing it
    /// to a rounder-looking 4096, which does not divide it.
    #[test]
    fn two_thousand_and_forty_eight_is_the_largest_block_that_divides_it() {
        let samples = u64::from(reference().samples());
        assert_eq!(samples % u64::from(BLOCK_SAMPLES), 0);
        assert_ne!(samples % (u64::from(BLOCK_SAMPLES) * 2), 0);
    }

    /// The elimination that makes `200` the tempo rather than a guess: the
    /// other products of these values put the loop nowhere near its real
    /// length. Half a second of tolerance is far tighter than the alternatives
    /// miss by.
    #[test]
    fn no_other_pairing_of_the_values_fits_the_audio() {
        let m = reference().meta.unwrap();
        let actual = f64::from(reference().samples()) / 44_100.0;

        let seconds = |beats: u32| f64::from(beats) * 60.0 / f64::from(m.tempo_bpm);
        assert!((seconds(7 * 4) - actual).abs() < 0.5, "7x4 must fit");
        assert!((seconds(7 * 8) - actual).abs() > 0.5, "7x8 must not fit");
        assert!((seconds(8 * 4) - actual).abs() > 0.5, "8x4 must not fit");
    }

    /// A plain WAV written by anything else has no vendor metadata, and saying
    /// so is more useful than inventing a default tempo for it.
    #[test]
    fn a_wav_without_the_label_parses_but_carries_no_metadata() {
        let mut bytes = LOOP0031;
        // Break the label, leaving a well-formed JUNK chunk of the right size.
        bytes[20] = b'X';
        let h = parse(&bytes).unwrap();
        assert!(h.meta.is_none());
        assert_eq!(h.tempo_agrees_with_audio(), None);
        // The audio is still fully described, which is the point of the chunk
        // being JUNK.
        assert_eq!(h.samples(), 370_688);
    }

    #[test]
    fn a_short_read_is_reported_as_truncation_not_as_a_bad_file() {
        for len in [0usize, 4, 11, 12, 40, 60, 80, 91] {
            let err = parse(&LOOP0031[..len]).unwrap_err();
            assert!(
                matches!(err, LoopError::Truncated { .. }),
                "{len} bytes gave {err:?}"
            );
        }
        assert!(parse(&LOOP0031).is_ok(), "the full prefix must suffice");
    }

    #[test]
    fn something_that_is_not_a_wav_is_refused() {
        let mut bytes = LOOP0031;
        bytes[0] = b'X';
        assert_eq!(parse(&bytes), Err(LoopError::NotRiffWave));
    }

    /// Odd-sized chunks are padded to an even boundary. No vendor file has one,
    /// but a reader that ignores the rule silently misreads every chunk after
    /// the first odd one, which is the kind of bug that surfaces as "some other
    /// person's loops don't work".
    #[test]
    fn an_odd_length_chunk_is_padded_before_the_next_one() {
        let mut bytes = alloc::vec::Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        // A three-byte chunk, padded to four.
        bytes.extend_from_slice(b"odd ");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 0]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&192_000u32.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&8u32.to_le_bytes());

        let h = parse(&bytes).unwrap();
        assert_eq!(h.format.sample_rate, 48_000);
        assert_eq!(h.format.channels, 2);
        assert_eq!(h.audio_bytes, 8);
        // Two channels of 16 bits is four bytes per frame.
        assert_eq!(h.samples(), 2);
    }

    #[test]
    fn a_zero_tempo_does_not_divide_by_zero() {
        let m = LoopMeta {
            version: 1,
            tempo_bpm: 0,
            length_first: 4,
            den: 8,
            length_second: 4,
            trailing: 0,
        };
        assert_eq!(m.nominal_samples(44_100), None);
        assert_eq!(m.expected_samples(44_100), None);
    }
}
