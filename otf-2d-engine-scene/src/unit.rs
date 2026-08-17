//! What a coordinate value of 1.0 means.

/// The physical meaning of one scene unit. Set once per scene, never per draw.
///
/// Raster backends ignore this entirely: the consumer has already baked scale
/// into its transforms and the output is pixels regardless. Vector backends
/// *require* it, because PDF's coordinate system is physical — an A4 page is
/// 595 × 842 points, and a backend that guesses produces a plausible-looking
/// document at the wrong physical size (Doc 02 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum SceneUnit {
    /// Unitless. The consumer owns all scaling. Raster targets only —
    /// vector backends reject this with `VectorError::UnitRequired` (D-19).
    #[default]
    Logical = 0,
    /// 1.0 is one device pixel.
    Pixel = 1,
    /// 1.0 is one point, i.e. 1/72 inch.
    Point = 2,
    /// 1.0 is one millimetre.
    Millimeter = 3,
}

impl SceneUnit {
    /// Inches expressed in points. Inches are a conversion, not a variant.
    #[inline]
    pub fn inches(v: f64) -> f64 {
        v * 72.0
    }

    /// Metres expressed in millimetres.
    #[inline]
    pub fn meters(v: f64) -> f64 {
        v * 1000.0
    }

    /// True when the unit denotes a physical size, so a vector backend can
    /// place it on paper.
    #[inline]
    pub fn is_physical(self) -> bool {
        matches!(self, SceneUnit::Point | SceneUnit::Millimeter)
    }

    /// The wire encoding, used by [`crate::Scene::to_bytes`].
    #[inline]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// Decodes [`SceneUnit::to_u8`], rejecting unknown values.
    #[inline]
    pub const fn from_u8(v: u8) -> Option<SceneUnit> {
        match v {
            0 => Some(SceneUnit::Logical),
            1 => Some(SceneUnit::Pixel),
            2 => Some(SceneUnit::Point),
            3 => Some(SceneUnit::Millimeter),
            _ => None,
        }
    }
}
