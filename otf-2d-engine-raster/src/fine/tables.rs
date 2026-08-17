//! Transfer-function tables for the u8 pipeline.
//!
//! Held by the caller and built once, rather than being `const` arrays: the
//! sRGB encode needs a power function, which is not available in a `const`
//! context, and checking in four thousand generated literals would put the
//! definition somewhere no test can see it.
//!
//! Every entry is `u32` even where a `u8` would fit. That is deliberate: the
//! SIMD kernel reads these tables with 32-bit gathers, and a second `u8` copy
//! for the scalar path would be a second thing to keep in step — the two paths
//! must read the *same* bytes for bit-identity to be structural.

use alloc::boxed::Box;

use otf_2d_engine_color::{SRGB8_TO_LINEAR, linear_to_srgb8};

/// Bits of fractional precision in a linear level.
pub const LINEAR_SHIFT: u32 = 12;

/// A fully lit channel, in fixed-point linear light.
pub const LINEAR_SCALE: u32 = 1 << LINEAR_SHIFT;

/// Distinct linear levels, including both endpoints.
pub const LINEAR_LEVELS: usize = LINEAR_SCALE as usize + 1;

/// sRGB and alpha conversion tables.
///
/// Roughly 18 KiB. Build one per process and lend it to every render; it is
/// immutable once built.
#[derive(Debug, Clone)]
pub struct FineTables {
    /// sRGB byte to linear level.
    decode: Box<[u32; 256]>,
    /// Alpha byte (and coverage byte) to linear level. Alpha carries no
    /// transfer function, so this is a plain scale.
    alpha: Box<[u32; 256]>,
    /// Linear level to sRGB byte, four bytes packed per word.
    ///
    /// Packed rather than one `u32` per level because this is the table the
    /// inner loop hits hardest, and at one word per level it is 16 KiB — which
    /// it loses from L1 to the megabytes of destination pixels streaming past
    /// it. Packed it is 4 KiB and stays resident, which on a Skylake-class
    /// core is the difference between a gather costing five cycles and twenty.
    encode: Box<[u32; ENCODE_WORDS]>,
}

/// Words needed to hold [`LINEAR_LEVELS`] packed bytes.
pub(crate) const ENCODE_WORDS: usize = LINEAR_LEVELS.div_ceil(4);

impl FineTables {
    /// Builds the tables.
    pub fn new() -> FineTables {
        let mut decode = Box::new([0u32; 256]);
        let mut alpha = Box::new([0u32; 256]);
        for byte in 0..256usize {
            let linear = SRGB8_TO_LINEAR[byte];
            decode[byte] = round_to_level(linear);
            alpha[byte] = ((byte as u32 * LINEAR_SCALE) + 127) / 255;
        }

        let mut encode = Box::new([0u32; ENCODE_WORDS]);
        for level in 0..LINEAR_LEVELS {
            let byte = linear_to_srgb8(level as f32 / LINEAR_SCALE as f32) as u32;
            encode[level / 4] |= byte << ((level % 4) * 8);
        }

        FineTables {
            decode,
            alpha,
            encode,
        }
    }

    /// sRGB byte to linear level.
    #[inline]
    pub fn decode(&self, byte: u8) -> u32 {
        self.decode[byte as usize]
    }

    /// Alpha byte to linear level.
    #[inline]
    pub fn decode_alpha(&self, byte: u8) -> u32 {
        self.alpha[byte as usize]
    }

    /// Coverage byte to linear level. Coverage is an area fraction, so it
    /// scales exactly like alpha.
    #[inline]
    pub fn coverage(&self, byte: u8) -> u32 {
        self.alpha[byte as usize]
    }

    /// Linear level to sRGB byte.
    #[inline]
    pub fn encode(&self, level: u32) -> u8 {
        let level = (level as usize).min(LINEAR_LEVELS - 1);
        (self.encode[level / 4] >> ((level % 4) * 8)) as u8
    }

    /// Linear level to alpha byte.
    #[inline]
    pub fn encode_alpha(&self, level: u32) -> u8 {
        let level = level.min(LINEAR_SCALE);
        (((level * 255) + (LINEAR_SCALE / 2)) >> LINEAR_SHIFT) as u8
    }

    /// The raw tables, for the SIMD kernel's gathers.
    #[inline]
    pub(crate) fn raw(&self) -> (&[u32; 256], &[u32; 256], &[u32; ENCODE_WORDS]) {
        (&self.decode, &self.alpha, &self.encode)
    }
}

impl Default for FineTables {
    fn default() -> FineTables {
        FineTables::new()
    }
}

/// A unit-interval linear value as a fixed-point level, rounded to nearest.
fn round_to_level(value: f32) -> u32 {
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    let scaled = value * LINEAR_SCALE as f32 + 0.5;
    if scaled >= LINEAR_SCALE as f32 {
        LINEAR_SCALE
    } else {
        scaled as u32
    }
}
