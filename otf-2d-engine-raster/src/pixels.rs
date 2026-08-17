//! The pixel target stage 6 writes into.
//!
//! `otf-2d-engine-raster` deliberately does not own a `Pixmap`: allocating and
//! owning the surface is the CPU backend's job (Doc 02 §7), and a consumer
//! rendering into a window surface or a shared-memory buffer must be able to
//! lend the bytes without a copy. Stage 6 only needs a borrowed, strided,
//! format-tagged view.

use crate::fine::{FineTables, LINEAR_SCALE};

/// Byte layout of a target pixel.
///
/// # What "premultiplied" means here
///
/// The stored byte is the **sRGB encoding of the premultiplied linear value**,
/// and alpha is linear. This is the convention that makes decode → blend →
/// encode exact, which is what lets an untouched pixel come back bit-identical
/// (D-32). For an opaque pixel — the case the u8 fast path exists for — it
/// coincides with every other premultiplied convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PixelFormat {
    #[default]
    Rgba8Premul,
    Bgra8Premul,
}

impl PixelFormat {
    /// Bytes per pixel.
    pub const fn bytes_per_pixel(self) -> usize {
        4
    }

    /// Where the red, green and blue channels sit. Alpha is always last.
    pub const fn channel_order(self) -> [usize; 3] {
        match self {
            PixelFormat::Rgba8Premul => [0, 1, 2],
            PixelFormat::Bgra8Premul => [2, 1, 0],
        }
    }
}

/// Why a byte slice cannot be a render target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetError {
    /// The slice is shorter than `stride * height`.
    TooSmall { needed: usize, found: usize },
    /// The stride does not cover `width` pixels.
    StrideTooSmall { needed: usize, found: usize },
}

impl core::fmt::Display for TargetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooSmall { needed, found } => {
                write!(f, "target needs {needed} bytes, got {found}")
            }
            Self::StrideTooSmall { needed, found } => {
                write!(f, "stride {found} does not cover a row of {needed} bytes")
            }
        }
    }
}

impl core::error::Error for TargetError {}

/// A borrowed, strided pixel buffer.
#[derive(Debug)]
pub struct TargetMut<'a> {
    data: &'a mut [u8],
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
}

impl<'a> TargetMut<'a> {
    /// A tightly packed target.
    pub fn new(
        data: &'a mut [u8],
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<TargetMut<'a>, TargetError> {
        let stride = width as usize * format.bytes_per_pixel();
        TargetMut::with_stride(data, width, height, stride, format)
    }

    /// A target whose rows are `stride` bytes apart.
    pub fn with_stride(
        data: &'a mut [u8],
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
    ) -> Result<TargetMut<'a>, TargetError> {
        let row = width as usize * format.bytes_per_pixel();
        if stride < row {
            return Err(TargetError::StrideTooSmall {
                needed: row,
                found: stride,
            });
        }
        let needed = stride
            .checked_mul(height as usize)
            .ok_or(TargetError::TooSmall {
                needed: usize::MAX,
                found: data.len(),
            })?;
        if data.len() < needed {
            return Err(TargetError::TooSmall {
                needed,
                found: data.len(),
            });
        }
        Ok(TargetMut {
            data,
            width,
            height,
            stride,
            format,
        })
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    #[inline]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// One row of pixels, or `None` past the bottom edge.
    #[inline]
    pub fn row_mut(&mut self, y: u32) -> Option<&mut [u8]> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.stride;
        let end = start + self.width as usize * self.format.bytes_per_pixel();
        self.data.get_mut(start..end)
    }

    /// Fills the whole target with one colour, encoded for its format.
    pub fn clear(&mut self, color: otf_2d_engine_color::Color, tables: &FineTables) {
        let bytes = encode_color(color, self.format, tables);
        for y in 0..self.height {
            let Some(row) = self.row_mut(y) else { continue };
            for pixel in row.chunks_exact_mut(4) {
                pixel.copy_from_slice(&bytes);
            }
        }
    }
}

/// Encodes a linear premultiplied colour into target bytes.
pub fn encode_color(
    color: otf_2d_engine_color::Color,
    format: PixelFormat,
    tables: &FineTables,
) -> [u8; 4] {
    let srgb = color.convert_to(otf_2d_engine_color::ColorSpace::Srgb);
    let premul = srgb.to_premul();
    let mut bytes = [0u8; 4];
    let order = format.channel_order();
    for (channel, &slot) in order.iter().enumerate() {
        let level = quantize(premul[channel]);
        bytes[slot] = tables.encode(level);
    }
    bytes[3] = tables.encode_alpha(quantize(premul[3]));
    bytes
}

/// A unit-interval value as a fixed-point linear level.
#[inline]
pub(crate) fn quantize(value: f32) -> u32 {
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
