use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ModelError;

/// Upper sanity bound (10 km²). Large enough for a rural *chácara*, small enough to catch
/// mis-parsed numbers such as phone numbers or prices.
pub const MAX_AREA_M2: u32 = 10_000_000;

/// A strictly positive surface area stored as integer hundredths of a square metre.
///
/// An unknown area is represented by `Fact::Unknown` elsewhere, never by a zero `Area`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Area(u32);

impl Area {
    pub fn from_centi_square_meters(centi_m2: u64) -> Result<Self, ModelError> {
        let max_centi = u64::from(MAX_AREA_M2) * 100;
        if centi_m2 == 0 || centi_m2 > max_centi {
            return Err(ModelError::InvalidArea {
                centi_m2,
                max_m2: MAX_AREA_M2,
            });
        }
        // Bounded by `max_centi` (1e9) which fits in u32.
        Ok(Self(
            u32::try_from(centi_m2).expect("bounded by MAX_AREA_M2 * 100"),
        ))
    }

    pub fn from_square_meters(m2: u32) -> Result<Self, ModelError> {
        Self::from_centi_square_meters(u64::from(m2) * 100)
    }

    #[must_use]
    pub const fn centi_square_meters(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for Area {
    type Error = ModelError;

    fn try_from(centi_m2: u32) -> Result<Self, Self::Error> {
        Self::from_centi_square_meters(u64::from(centi_m2))
    }
}

impl From<Area> for u32 {
    fn from(area: Area) -> Self {
        area.0
    }
}

/// Formats as `180 m²`, `180,5 m²` or `180,55 m²`.
impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (whole, frac) = (self.0 / 100, self.0 % 100);
        match frac {
            0 => write!(f, "{whole} m²"),
            f2 if f2 % 10 == 0 => write!(f, "{whole},{} m²", f2 / 10),
            f2 => write!(f, "{whole},{f2:02} m²"),
        }
    }
}

macro_rules! typed_area {
    ($( $(#[$meta:meta])* $name:ident ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
            #[serde(transparent)]
            pub struct $name(Area);

            impl $name {
                #[must_use]
                pub const fn new(area: Area) -> Self {
                    Self(area)
                }

                #[must_use]
                pub const fn area(self) -> Area {
                    self.0
                }
            }
        )+
    };
}

typed_area! {
    /// Area of the land plot (*lote* / *terreno*).
    LotArea,
    /// Constructed / built area (*área construída*).
    BuiltArea,
    /// Explicitly stated usable, internal or private area (*área útil / interna / privativa*).
    UsableArea,
    /// An area the advertisement gives without saying which kind it is. It is never promoted
    /// to lot or built area.
    UnspecifiedArea,
}

string_enum! {
    /// Which kind of area a number measures. Kept separate on purpose; see [`ClassifiedArea`].
    pub enum AreaKind {
        Lot => "lot",
        Built => "built",
        Usable => "usable",
        Unspecified => "unspecified",
    }
}

/// An area together with its kind. Consumers that need a specific kind must match on the
/// variant, so lot and built area can never be substituted for one another silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "area", rename_all = "snake_case")]
pub enum ClassifiedArea {
    Lot(LotArea),
    Built(BuiltArea),
    Usable(UsableArea),
    Unspecified(UnspecifiedArea),
}

impl ClassifiedArea {
    #[must_use]
    pub const fn new(kind: AreaKind, area: Area) -> Self {
        match kind {
            AreaKind::Lot => Self::Lot(LotArea::new(area)),
            AreaKind::Built => Self::Built(BuiltArea::new(area)),
            AreaKind::Usable => Self::Usable(UsableArea::new(area)),
            AreaKind::Unspecified => Self::Unspecified(UnspecifiedArea::new(area)),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AreaKind {
        match self {
            Self::Lot(_) => AreaKind::Lot,
            Self::Built(_) => AreaKind::Built,
            Self::Usable(_) => AreaKind::Usable,
            Self::Unspecified(_) => AreaKind::Unspecified,
        }
    }

    #[must_use]
    pub const fn area(&self) -> Area {
        match self {
            Self::Lot(a) => a.area(),
            Self::Built(a) => a.area(),
            Self::Usable(a) => a.area(),
            Self::Unspecified(a) => a.area(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_and_absurd_areas() {
        assert!(Area::from_centi_square_meters(0).is_err());
        assert!(Area::from_square_meters(0).is_err());
        assert!(Area::from_square_meters(MAX_AREA_M2 + 1).is_err());
        assert!(Area::from_square_meters(MAX_AREA_M2).is_ok());
    }

    #[test]
    fn displays_without_trailing_zeros() {
        assert_eq!(Area::from_square_meters(180).unwrap().to_string(), "180 m²");
        assert_eq!(
            Area::from_centi_square_meters(18_050).unwrap().to_string(),
            "180,5 m²"
        );
        assert_eq!(
            Area::from_centi_square_meters(18_005).unwrap().to_string(),
            "180,05 m²"
        );
    }

    #[test]
    fn classified_area_keeps_its_kind() {
        let a = Area::from_square_meters(300).unwrap();
        for kind in AreaKind::ALL {
            let classified = ClassifiedArea::new(*kind, a);
            assert_eq!(classified.kind(), *kind);
            assert_eq!(classified.area(), a);
        }
        assert_ne!(
            ClassifiedArea::new(AreaKind::Lot, a),
            ClassifiedArea::new(AreaKind::Built, a),
            "equal numbers of different kinds must not compare equal"
        );
    }

    #[test]
    fn serde_round_trip() {
        let c = ClassifiedArea::new(AreaKind::Built, Area::from_square_meters(180).unwrap());
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"kind":"built","area":18000}"#);
        assert_eq!(serde_json::from_str::<ClassifiedArea>(&json).unwrap(), c);
    }
}
