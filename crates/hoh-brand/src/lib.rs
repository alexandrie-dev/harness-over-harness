//! Stateless nominal primitive constructors. Domain owners validate their values.
extern crate self as hoh_brand;
#[doc(hidden)]
pub use serde as __serde;

pub trait Branded: From<String> + AsRef<str> {}
pub trait BrandedNumber: Sized {
    type Number;
    fn from_number(value: Self::Number) -> Self;
}

pub fn brand_string<T: Branded>(value: impl Into<String>) -> T {
    T::from(value.into())
}
pub fn brand_number<T: BrandedNumber>(value: T::Number) -> T {
    T::from_number(value)
}

/// Declare a domain-owned transparent string newtype: `branded_string!(pub SessionId);`.
#[macro_export]
macro_rules! branded_string {
    ($vis:vis $name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            $crate::__serde::Serialize, $crate::__serde::Deserialize)]
        #[serde(transparent, crate = "hoh_brand::__serde")]
        $vis struct $name(String);
        impl $name { pub fn as_str(&self) -> &str { &self.0 } }
        impl From<String> for $name { fn from(value: String) -> Self { Self(value) } }
        impl From<&str> for $name { fn from(value: &str) -> Self { Self(value.into()) } }
        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl AsRef<str> for $name { fn as_ref(&self) -> &str { &self.0 } }
        impl std::borrow::Borrow<str> for $name { fn borrow(&self) -> &str { &self.0 } }
        impl PartialEq<str> for $name { fn eq(&self, other: &str) -> bool { self.0 == other } }
        impl PartialEq<&str> for $name { fn eq(&self, other: &&str) -> bool { self.0 == *other } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
        }
        impl $crate::Branded for $name {}
    };
}

/// Declare an integer brand usable as a map key; owners choose an integer representation (normally `u64`).
#[macro_export]
macro_rules! branded_number {
    ($vis:vis $name:ident, $number:ty) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            $crate::__serde::Serialize, $crate::__serde::Deserialize)]
        #[serde(transparent, crate = "hoh_brand::__serde")]
        $vis struct $name($number);
        impl $crate::BrandedNumber for $name {
            type Number = $number;
            fn from_number(value: $number) -> Self { Self(value) }
        }
        impl From<$name> for $number { fn from(value: $name) -> Self { value.0 } }
    };
}

/// Brands with repeated source declarations share one concrete Rust type.
pub mod ids;
