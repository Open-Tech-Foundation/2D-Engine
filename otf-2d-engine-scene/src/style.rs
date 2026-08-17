//! Paint, stroke style and glyph types — the value types a consumer passes to
//! the encoder. These are public API (Doc 02 §4, §5, §6); the arena records
//! they encode into are not.

use otf_2d_engine_color::Color;
use otf_2d_engine_geom::Point;
use smallvec::SmallVec;

use crate::handles::{ImageRef, StopsRef, TransformRef, VariationsRef};

/// Which points a fill considers inside the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum FillRule {
    /// Non-zero winding. The default, and what most authoring tools mean.
    #[default]
    NonZero = 0,
    /// Even-odd winding.
    EvenOdd = 1,
}

/// What happens outside a gradient's `[0, 1]` parameter range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Extend {
    /// Hold the end stops. The default.
    #[default]
    Pad = 0,
    Repeat = 1,
    Reflect = 2,
}

/// How image pixels are reconstructed between sample points.
///
/// Bicubic and mipmapped sampling are deferred to P5 (Doc 02 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Sampling {
    Nearest = 0,
    #[default]
    Bilinear = 1,
}

/// One stop in a gradient ramp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    /// Position along the gradient, normally in `[0, 1]`.
    pub offset: f32,
    pub color: Color,
}

impl ColorStop {
    pub const fn new(offset: f32, color: Color) -> Self {
        Self { offset, color }
    }
}

/// What a shape is filled or stroked with.
///
/// Conic and sweep gradients, mesh gradients and pattern paints are deferred
/// to P5 (Doc 02 §4). Stops are interned into the scene first, which is why
/// gradients carry a [`StopsRef`] rather than a slice — the IR holds no
/// pointers (I-2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Solid(Color),
    LinearGradient {
        start: Point,
        end: Point,
        stops: StopsRef,
        extend: Extend,
    },
    RadialGradient {
        center: Point,
        radius: f64,
        /// The focal point for a two-point conical gradient. `None` centres it.
        focal: Option<Point>,
        stops: StopsRef,
        extend: Extend,
    },
    Image {
        image: ImageRef,
        sampling: Sampling,
        transform: TransformRef,
    },
}

impl Paint {
    /// A solid opaque colour from an `0xRRGGBBAA` literal.
    pub fn hex(rgba: u32) -> Paint {
        Paint::Solid(Color::from_rgba8_hex(rgba))
    }

    /// True when this paint covers everything it touches, so anything beneath
    /// it can be skipped. Only solids can answer this without evaluating.
    pub fn is_opaque(&self) -> bool {
        matches!(self, Paint::Solid(c) if c.is_opaque())
    }
}

/// How a stroke turns a corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Join {
    /// Extend the outer edges until they meet, falling back to a bevel when
    /// the resulting spike exceeds `limit` times the stroke width.
    Miter {
        limit: f32,
    },
    #[default]
    Round,
    Bevel,
}

impl Join {
    /// The CSS and SVG default miter limit.
    pub const DEFAULT_MITER: Join = Join::Miter { limit: 4.0 };
}

/// How a stroke ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Cap {
    /// Stop at the endpoint. The default.
    #[default]
    Butt = 0,
    Round = 1,
    /// Extend by half the stroke width.
    Square = 2,
}

/// A dash pattern.
///
/// Dashing is a path-to-path transform applied in stage 3 before offset
/// expansion, never a rasterizer feature (Doc 01 §4, §8).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dash {
    /// Alternating on and off lengths. An odd-length pattern repeats to even.
    pub pattern: SmallVec<[f32; 4]>,
    /// How far into the pattern the first subpath starts.
    pub offset: f32,
}

impl Dash {
    pub fn new(pattern: &[f32], offset: f32) -> Self {
        Self {
            pattern: SmallVec::from_slice(pattern),
            offset,
        }
    }

    /// True when the pattern would draw nothing or draw everything, in which
    /// case stage 3 can skip dashing entirely.
    pub fn is_degenerate(&self) -> bool {
        self.pattern.is_empty() || self.pattern.iter().all(|&v| v <= 0.0)
    }
}

/// How a path is stroked.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub join: Join,
    pub start_cap: Cap,
    pub end_cap: Cap,
    pub dash: Option<Dash>,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            join: Join::DEFAULT_MITER,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            dash: None,
        }
    }
}

impl StrokeStyle {
    /// A stroke of the given width with the CSS defaults for everything else.
    pub fn new(width: f32) -> Self {
        Self {
            width,
            ..Self::default()
        }
    }

    pub fn with_join(mut self, join: Join) -> Self {
        self.join = join;
        self
    }

    /// Sets both caps.
    pub fn with_caps(mut self, cap: Cap) -> Self {
        self.start_cap = cap;
        self.end_cap = cap;
        self
    }

    pub fn with_dash(mut self, dash: Dash) -> Self {
        self.dash = Some(dash);
        self
    }
}

/// A positioned glyph.
///
/// `id` is an index within the font, not a Unicode codepoint: 2D-Engine takes
/// shaped glyph runs and never strings (Doc 02 §6).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Glyph {
    /// Glyph index within the font — *not* a codepoint.
    pub id: u32,
    /// Position in run-local space.
    pub x: f32,
    pub y: f32,
}

impl Glyph {
    pub const fn new(id: u32, x: f32, y: f32) -> Self {
        Self { id, x, y }
    }
}

/// Outline hinting. v1 supports `None` only (D-05): hinting was removed
/// because HiDPI is universal and the interpreter is a large legacy surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
#[non_exhaustive]
pub enum Hinting {
    #[default]
    None = 0,
}

/// Options affecting how a glyph's outline is extracted.
///
/// These change the outline, not the shaping — shaping is the consumer's job
/// (Doc 02 §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphOptions {
    pub hinting: Hinting,
    /// Emboldening in em units. Zero for none.
    pub synthetic_bold: f32,
    /// Horizontal shear tangent, for synthetic italic. Zero for none.
    pub synthetic_skew: f32,
    /// Variable-font axis coordinates, interned into the scene.
    pub variations: VariationsRef,
}

impl Default for GlyphOptions {
    fn default() -> Self {
        Self {
            hinting: Hinting::None,
            synthetic_bold: 0.0,
            synthetic_skew: 0.0,
            variations: VariationsRef::NONE,
        }
    }
}
