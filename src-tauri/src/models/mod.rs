//! Domain types.
//!
//! Everything here is plain data with validated constructors; nothing touches SQLite or the
//! network. The types are deliberately narrow so that incompatible concepts cannot be mixed by
//! accident: a [`price::FullPrice`] is not an [`price::Agio`], a [`area::LotArea`] is not a
//! [`area::BuiltArea`], and an unknown value is [`characteristics::Fact::Unknown`], never zero.

/// Declares a fieldless enum with a stable text representation shared by serde, `Display`,
/// `FromStr` and SQLite `CHECK` constraints. The `ALL` constant lets tests prove that the Rust
/// enum and the SQL constraint accept exactly the same values.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        $vis enum $name {
            $( $(#[$vmeta])* #[serde(rename = $text)] $variant, )+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// Stable text form used in SQLite columns and JSON.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $text ),+ }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::error::ModelError;

            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                match s {
                    $( $text => Ok(Self::$variant), )+
                    other => Err($crate::error::ModelError::UnknownVariant {
                        kind: stringify!($name),
                        value: other.to_owned(),
                    }),
                }
            }
        }
    };
}

pub mod area;
pub mod characteristics;
pub mod collection;
pub mod confidence;
pub mod evidence;
pub mod image;
pub mod listing;
pub mod matching;
pub mod money;
pub mod price;
pub mod region;
