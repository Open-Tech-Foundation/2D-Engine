//! The plain-old-data records that make up the arena buffers.
//!
//! Every type here is `#[repr(C)]` and `Pod`: no padding, no enums, no
//! pointers. That is what makes the whole scene castable to bytes in one pass
//! (Doc 02 §2) and what keeps `Scene: Send + Sync` free rather than asserted.
//!
//! The encoding format is **not public API** (Doc 02 §8). These types are
//! visible so the raster crate can read them, not so consumers can write them.

use bytemuck::{Pod, Zeroable};

/// What one draw command is.
///
/// Stored as a `u8` in [`DrawTag`] rather than a Rust enum, because an enum
/// with an invalid discriminant is undefined behaviour and a deserialised
/// scene is untrusted bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DrawKind {
    Fill = 0,
    Stroke = 1,
    Glyphs = 2,
    Image = 3,
    PushLayer = 4,
    PopLayer = 5,
}

impl DrawKind {
    #[inline]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn from_u8(v: u8) -> Option<DrawKind> {
        match v {
            0 => Some(DrawKind::Fill),
            1 => Some(DrawKind::Stroke),
            2 => Some(DrawKind::Glyphs),
            3 => Some(DrawKind::Image),
            4 => Some(DrawKind::PushLayer),
            5 => Some(DrawKind::PopLayer),
            _ => None,
        }
    }
}

/// One draw command, in submission order.
///
/// 20 bytes, no padding. Which arena `payload` and `aux` index depends on
/// `kind`; see [`crate::Scene`]'s accessors, which do the interpretation once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct DrawTag {
    /// A [`DrawKind`] discriminant.
    pub kind: u8,
    /// Kind-specific bits. `Fill`: bit 0 is the fill rule.
    pub flags: u8,
    /// Reserved for future kind-specific data. Kept so the record stays a
    /// multiple of four bytes without an implicit padding hole.
    pub reserved: u16,
    /// The resolved transform for this draw.
    pub transform: u32,
    /// The paint, or `NO_REF` for kinds that carry none.
    pub paint: u32,
    /// `PathRef` for fills and strokes, `GlyphRunRef` for glyphs, `ImageRef`
    /// for images, `LayerRef` for layer boundaries.
    pub payload: u32,
    /// `StrokeRef` for strokes. Unused (`NO_REF`) otherwise.
    pub aux: u32,
}

/// Bit 0 of [`DrawTag::flags`] on a `Fill`: set means even-odd.
pub const FLAG_EVEN_ODD: u8 = 1 << 0;

/// Which recognised primitive a path is, if any (Doc 02 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum ShapeKind {
    #[default]
    General = 0,
    Rect = 1,
    RoundedRect = 2,
}

impl ShapeKind {
    #[inline]
    pub const fn to_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub const fn from_u32(v: u32) -> Option<ShapeKind> {
        match v {
            0 => Some(ShapeKind::General),
            1 => Some(ShapeKind::Rect),
            2 => Some(ShapeKind::RoundedRect),
            _ => None,
        }
    }
}

/// Where one path's verbs and points live, plus its precomputed bounds.
///
/// 88 bytes. `bounds` doubles as the rectangle for `Rect` and `RoundedRect`
/// shapes, which is exact: `PathBuilder::rounded_rect` clamps its radii so the
/// control bounds equal the rect it was built from.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct PathDesc {
    /// `[x0, y0, x1, y1]` control-point bounds in scene space.
    pub bounds: [f64; 4],
    /// Corner radii for `ShapeKind::RoundedRect`, clockwise from top left.
    /// Zero otherwise.
    pub radii: [f64; 4],
    /// Offset into `path_verbs`.
    pub verb_offset: u32,
    pub verb_len: u32,
    /// Offset into `path_data`, counted in `f64` elements — two per point.
    pub point_offset: u32,
    pub point_len: u32,
    /// A [`ShapeKind`] discriminant.
    pub shape: u32,
    /// Reserved. Keeps the record eight-byte aligned with no padding hole.
    pub reserved: u32,
}

/// An affine, stored as the six coefficients `Affine` exposes.
///
/// `f64` per D-21: the scene is device-independent, and stage 2 is where
/// coordinates become bounded enough for `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(transparent)]
pub struct TransformRec(pub [f64; 6]);

/// What kind of paint a [`PaintDesc`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum PaintKind {
    #[default]
    Solid = 0,
    LinearGradient = 1,
    RadialGradient = 2,
    Image = 3,
}

impl PaintKind {
    #[inline]
    pub const fn to_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub const fn from_u32(v: u32) -> Option<PaintKind> {
        match v {
            0 => Some(PaintKind::Solid),
            1 => Some(PaintKind::LinearGradient),
            2 => Some(PaintKind::RadialGradient),
            3 => Some(PaintKind::Image),
            _ => None,
        }
    }
}

/// Set in [`PaintDesc::flags`] when a radial gradient has an explicit focal
/// point. Without it the focal coincides with the centre.
pub const PAINT_FLAG_HAS_FOCAL: u32 = 1 << 0;

/// One paint. 96 bytes, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct PaintDesc {
    /// A [`PaintKind`] discriminant.
    pub kind: u32,
    /// [`crate::Extend`] for gradients, [`crate::Sampling`] for images.
    pub mode: u32,
    /// The [`otf_2d_engine_color::ColorSpace`] of `color` and of the stops.
    pub color_space: u32,
    pub flags: u32,
    /// Premultiplied linear RGBA for `Solid`. Unused otherwise.
    pub color: [f32; 4],
    /// Linear: `[start.x, start.y, end.x, end.y, _, _]`.
    /// Radial: `[center.x, center.y, radius, focal.x, focal.y, _]`.
    pub geometry: [f64; 6],
    /// Offset into `stops`, for gradients.
    pub stops_offset: u32,
    pub stops_len: u32,
    /// `ImageRef` for image paints.
    pub image: u32,
    /// `TransformRef` for image paints.
    pub transform: u32,
}

/// One gradient stop. 24 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct ColorStopRec {
    pub offset: f32,
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
    pub color_space: u32,
}

/// One stroke style. 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct StrokeDesc {
    pub width: f32,
    /// Meaningful only when `join` is miter.
    pub miter_limit: f32,
    /// 0 miter, 1 round, 2 bevel.
    pub join: u32,
    /// A [`crate::Cap`] discriminant.
    pub start_cap: u32,
    pub end_cap: u32,
    pub dash_offset: f32,
    /// Offset into `dash_data`.
    pub dash_offset_index: u32,
    pub dash_len: u32,
}

/// Join discriminants as stored in [`StrokeDesc::join`].
pub const JOIN_MITER: u32 = 0;
pub const JOIN_ROUND: u32 = 1;
pub const JOIN_BEVEL: u32 = 2;

/// One glyph run. 40 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct GlyphRunDesc {
    /// `FontRef` into the caller's registry.
    pub font: u32,
    /// Em size in scene units.
    pub size: f32,
    /// Offset into `glyphs`.
    pub glyph_offset: u32,
    pub glyph_len: u32,
    /// Offset into `variations`.
    pub variations_offset: u32,
    pub variations_len: u32,
    pub synthetic_bold: f32,
    pub synthetic_skew: f32,
    /// A [`crate::Hinting`] discriminant. Always `0` in v1 (D-05).
    pub hinting: u32,
    pub reserved: u32,
}

/// One positioned glyph. 12 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct GlyphRec {
    /// Glyph index within the font — not a codepoint.
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// One layer boundary. 24 bytes.
///
/// A push and its pop share a record; `pop_tag` is filled in when the pop is
/// encoded, which is what lets stage 2 resolve layer extents without walking
/// a tree.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct LayerDesc {
    /// A [`otf_2d_engine_color::BlendMode`] discriminant.
    pub blend: u32,
    /// Group opacity.
    pub alpha: f32,
    /// `PathRef` of the clip, or `NO_REF` for an unclipped layer.
    pub clip_path: u32,
    /// The transform the clip is expressed in.
    pub transform: u32,
    /// Index of the `PushLayer` tag that opened this layer.
    pub push_tag: u32,
    /// Index of the `PopLayer` tag that closed it, or `NO_REF` while open.
    pub pop_tag: u32,
}

/// One reusable subtree (Doc 03 §3).
///
/// Reserved: written by `push_node`, consumed by the node cache in M6. The
/// hash lives in a parallel `node_hashes` buffer, structure-of-arrays style,
/// because the cache compares hashes without touching anything else here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct NodeDesc {
    /// The caller-supplied [`crate::NodeId`].
    pub id: u64,
    /// First draw tag this node contributed.
    pub tag_offset: u32,
    pub tag_len: u32,
    /// Enclosing node index, or `NO_REF` at the top level.
    pub parent: u32,
    pub reserved: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    /// `Pod` already rejects padding, but pinning the sizes makes an
    /// accidental layout change visible in the diff — the arena layout is
    /// frozen at the M1 gate and cannot be retrofitted.
    #[test]
    fn record_sizes_are_pinned() {
        assert_eq!(size_of::<DrawTag>(), 20);
        assert_eq!(size_of::<PathDesc>(), 88);
        assert_eq!(size_of::<TransformRec>(), 48);
        assert_eq!(size_of::<PaintDesc>(), 96);
        assert_eq!(size_of::<ColorStopRec>(), 24);
        assert_eq!(size_of::<StrokeDesc>(), 32);
        assert_eq!(size_of::<GlyphRunDesc>(), 40);
        assert_eq!(size_of::<GlyphRec>(), 12);
        assert_eq!(size_of::<LayerDesc>(), 24);
        assert_eq!(size_of::<NodeDesc>(), 24);
    }

    #[test]
    fn records_never_need_more_than_eight_byte_alignment() {
        // Serialisation writes buffers back to back; anything stricter would
        // need explicit padding in the wire format.
        assert!(align_of::<DrawTag>() <= 8);
        assert!(align_of::<PathDesc>() <= 8);
        assert!(align_of::<PaintDesc>() <= 8);
        assert!(align_of::<TransformRec>() <= 8);
    }

    #[test]
    fn discriminants_round_trip_and_reject_unknown_values() {
        for kind in [
            DrawKind::Fill,
            DrawKind::Stroke,
            DrawKind::Glyphs,
            DrawKind::Image,
            DrawKind::PushLayer,
            DrawKind::PopLayer,
        ] {
            assert_eq!(DrawKind::from_u8(kind.to_u8()), Some(kind));
        }
        assert_eq!(DrawKind::from_u8(6), None);
        assert_eq!(DrawKind::from_u8(255), None);

        for kind in [ShapeKind::General, ShapeKind::Rect, ShapeKind::RoundedRect] {
            assert_eq!(ShapeKind::from_u32(kind.to_u32()), Some(kind));
        }
        assert_eq!(ShapeKind::from_u32(3), None);

        for kind in [
            PaintKind::Solid,
            PaintKind::LinearGradient,
            PaintKind::RadialGradient,
            PaintKind::Image,
        ] {
            assert_eq!(PaintKind::from_u32(kind.to_u32()), Some(kind));
        }
        assert_eq!(PaintKind::from_u32(4), None);
    }
}
