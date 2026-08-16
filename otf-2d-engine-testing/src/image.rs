//! The image type golden cases produce and reference PNGs decode into.
//!
//! Storage is straight-alpha RGBA8 because that is what a PNG holds. Engine
//! targets are premultiplied (Doc 01 §7); conversion happens at the harness
//! boundary, not here.

use std::fmt;
use std::path::Path;

/// An 8-bit RGBA image with straight (non-premultiplied) alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    data: Vec<u8>,
}

/// Why an image could not be read or written.
#[derive(Debug)]
pub enum ImageError {
    Io(std::io::Error),
    Decode(png::DecodingError),
    Encode(png::EncodingError),
    /// The pixel buffer length does not match `width * height * 4`.
    BadLength {
        width: u32,
        height: u32,
        len: usize,
    },
    /// The PNG is a colour type or bit depth the harness does not accept.
    UnsupportedFormat(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Decode(e) => write!(f, "png decode: {e}"),
            Self::Encode(e) => write!(f, "png encode: {e}"),
            Self::BadLength { width, height, len } => write!(
                f,
                "buffer of {len} bytes does not match {width}x{height} RGBA8 \
                 (expected {})",
                *width as usize * *height as usize * 4
            ),
            Self::UnsupportedFormat(s) => write!(f, "unsupported PNG format: {s}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<png::DecodingError> for ImageError {
    fn from(e: png::DecodingError) -> Self {
        Self::Decode(e)
    }
}

impl From<png::EncodingError> for ImageError {
    fn from(e: png::EncodingError) -> Self {
        Self::Encode(e)
    }
}

impl Image {
    /// Wraps an existing RGBA8 buffer.
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self, ImageError> {
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return Err(ImageError::BadLength {
                width,
                height,
                len: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// A fully transparent image.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The RGBA pixel at `(x, y)`. Panics if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) outside {}x{}",
            self.width,
            self.height
        );
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) outside {}x{}",
            self.width,
            self.height
        );
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// Reads an 8-bit PNG, expanding greyscale and palette forms to RGBA8.
    pub fn read_png(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        let file = std::fs::File::open(path.as_ref())?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::EXPAND);
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader.next_frame(&mut buf)?;

        if info.bit_depth != png::BitDepth::Eight {
            return Err(ImageError::UnsupportedFormat(format!(
                "bit depth {:?}; golden references are 8-bit",
                info.bit_depth
            )));
        }
        let data = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            other => {
                return Err(ImageError::UnsupportedFormat(format!(
                    "colour type {other:?} after alpha expansion"
                )));
            }
        };
        Self::from_rgba8(info.width, info.height, data)
    }

    /// Writes an 8-bit RGBA PNG, creating parent directories as needed.
    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<(), ImageError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.data)?;
        writer.finish()?;
        Ok(())
    }

    /// Compares against `other`, returning `None` when the two are identical.
    ///
    /// Differing dimensions are a mismatch with no per-pixel detail.
    pub fn compare(&self, other: &Image) -> Option<Mismatch> {
        if self.width != other.width || self.height != other.height {
            return Some(Mismatch {
                dimensions: Some(((self.width, self.height), (other.width, other.height))),
                differing_pixels: 0,
                max_channel_delta: 0,
                first_difference: None,
            });
        }

        let mut differing = 0u64;
        let mut max_delta = 0u8;
        let mut first = None;
        for i in 0..(self.width as usize * self.height as usize) {
            let a = &self.data[i * 4..i * 4 + 4];
            let b = &other.data[i * 4..i * 4 + 4];
            if a == b {
                continue;
            }
            differing += 1;
            for c in 0..4 {
                max_delta = max_delta.max(a[c].abs_diff(b[c]));
            }
            if first.is_none() {
                let (x, y) = (i as u32 % self.width, i as u32 / self.width);
                let (mut pa, mut pb) = ([0u8; 4], [0u8; 4]);
                pa.copy_from_slice(a);
                pb.copy_from_slice(b);
                first = Some(PixelDifference {
                    x,
                    y,
                    actual: pa,
                    expected: pb,
                });
            }
        }

        if differing == 0 {
            None
        } else {
            Some(Mismatch {
                dimensions: None,
                differing_pixels: differing,
                max_channel_delta: max_delta,
                first_difference: first,
            })
        }
    }

    /// A visual diff: matching pixels dimmed, differing pixels magenta.
    ///
    /// Returns `None` when the dimensions differ, since there is nothing to
    /// overlay.
    pub fn diff_image(&self, other: &Image) -> Option<Image> {
        if self.width != other.width || self.height != other.height {
            return None;
        }
        let mut out = Image::new(self.width, self.height);
        for i in 0..(self.width as usize * self.height as usize) {
            let a = &self.data[i * 4..i * 4 + 4];
            let b = &other.data[i * 4..i * 4 + 4];
            let px = if a == b {
                // Desaturate and darken so differences pop.
                let grey = ((a[0] as u32 + a[1] as u32 + a[2] as u32) / 3 / 3) as u8;
                [grey, grey, grey, 255]
            } else {
                [255, 0, 255, 255]
            };
            out.data[i * 4..i * 4 + 4].copy_from_slice(&px);
        }
        Some(out)
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// How two images differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// `Some((actual, expected))` when the dimensions themselves differ.
    pub dimensions: Option<((u32, u32), (u32, u32))>,
    pub differing_pixels: u64,
    pub max_channel_delta: u8,
    pub first_difference: Option<PixelDifference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelDifference {
    pub x: u32,
    pub y: u32,
    pub actual: [u8; 4],
    pub expected: [u8; 4],
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(((aw, ah), (ew, eh))) = self.dimensions {
            return write!(f, "dimensions differ: got {aw}x{ah}, expected {ew}x{eh}");
        }
        write!(
            f,
            "{} pixel(s) differ, max channel delta {}",
            self.differing_pixels, self.max_channel_delta
        )?;
        if let Some(d) = self.first_difference {
            write!(
                f,
                "; first at ({}, {}): got {:?}, expected {:?}",
                d.x, d.y, d.actual, d.expected
            )?;
        }
        Ok(())
    }
}
