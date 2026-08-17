//! The CPU render target (Doc 02 §7).

use alloc::vec;
use alloc::vec::Vec;

use otf_2d_engine_color::Color;
use otf_2d_engine_raster::{FineTables, PixelFormat, TargetMut, encode_color};

/// An owned pixel buffer.
///
/// 2D-Engine never allocates the final target unless asked: a consumer
/// rendering into a window surface or a shared-memory buffer lends its bytes
/// through [`Pixmap::borrowed`] instead, and no copy happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl Pixmap {
    /// A transparent pixmap.
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Pixmap {
        let len = width as usize * height as usize * format.bytes_per_pixel();
        Pixmap {
            data: vec![0; len],
            width,
            height,
            format,
        }
    }

    /// Wraps caller-owned bytes without copying.
    ///
    /// The borrowed form of this is [`TargetMut`], which is what the renderer
    /// actually takes; this is the owning convenience.
    pub fn borrowed<'a>(
        data: &'a mut [u8],
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<TargetMut<'a>, otf_2d_engine_raster::TargetError> {
        TargetMut::new(data, width, height, format)
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
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The bytes, given up.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// A borrowed view for the renderer to write into.
    pub fn as_target(&mut self) -> TargetMut<'_> {
        TargetMut::new(&mut self.data, self.width, self.height, self.format)
            .expect("a pixmap is always large enough for itself")
    }

    /// Overwrites every pixel.
    pub fn fill(&mut self, color: Color, tables: &FineTables) {
        let bytes = encode_color(color, self.format, tables);
        for pixel in self.data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&bytes);
        }
    }

    /// One pixel's bytes, for tests and for inspection.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ])
    }
}
