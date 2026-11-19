//! Root Option::None models JS undefined; unit/null and nested JSON stay distinct.
use serde::{Serialize, Serializer, ser::*};
use serde_json::{Error, Value};
type JsonSerializer = serde_json::value::Serializer;
fn base() -> JsonSerializer {
    serde_json::value::Serializer
}

pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Option<Value>, Error> {
    value.serialize(Root)
}
struct Root;
struct Finish<T>(T);

macro_rules! scalar {
    ($method:ident, $ty:ty) => {
        fn $method(self, value: $ty) -> Result<Self::Ok, Self::Error> {
            base().$method(value).map(Some)
        }
    };
}
impl Serializer for Root {
    type Ok = Option<Value>;
    type Error = Error;
    type SerializeSeq = Finish<<JsonSerializer as Serializer>::SerializeSeq>;
    type SerializeTuple = Finish<<JsonSerializer as Serializer>::SerializeTuple>;
    type SerializeTupleStruct = Finish<<JsonSerializer as Serializer>::SerializeTupleStruct>;
    type SerializeTupleVariant = Finish<<JsonSerializer as Serializer>::SerializeTupleVariant>;
    type SerializeMap = Finish<<JsonSerializer as Serializer>::SerializeMap>;
    type SerializeStruct = Finish<<JsonSerializer as Serializer>::SerializeStruct>;
    type SerializeStructVariant = Finish<<JsonSerializer as Serializer>::SerializeStructVariant>;
    scalar!(serialize_bool, bool);
    scalar!(serialize_i8, i8);
    scalar!(serialize_i16, i16);
    scalar!(serialize_i32, i32);
    scalar!(serialize_i64, i64);
    scalar!(serialize_i128, i128);
    scalar!(serialize_u8, u8);
    scalar!(serialize_u16, u16);
    scalar!(serialize_u32, u32);
    scalar!(serialize_u64, u64);
    scalar!(serialize_u128, u128);
    scalar!(serialize_f32, f32);
    scalar!(serialize_f64, f64);
    scalar!(serialize_char, char);
    scalar!(serialize_str, &str);
    scalar!(serialize_bytes, &[u8]);
    fn serialize_none(self) -> Result<Self::Ok, Error> {
        Ok(None)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Error> {
        base().serialize_unit().map(Some)
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Error> {
        base().serialize_unit_struct(name).map(Some)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Error> {
        base()
            .serialize_unit_variant(name, index, variant)
            .map(Some)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Error> {
        base()
            .serialize_newtype_variant(name, index, variant, value)
            .map(Some)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        base().serialize_seq(len).map(Finish)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Error> {
        base().serialize_tuple(len).map(Finish)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        base().serialize_tuple_struct(name, len).map(Finish)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        base()
            .serialize_tuple_variant(name, index, variant, len)
            .map(Finish)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        base().serialize_map(len).map(Finish)
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        base().serialize_struct(name, len).map(Finish)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        base()
            .serialize_struct_variant(name, index, variant, len)
            .map(Finish)
    }
}
macro_rules! sequence {
    ($trait:ident,$method:ident) => {
        impl<T: $trait<Ok = Value, Error = Error>> $trait for Finish<T> {
            type Ok = Option<Value>;
            type Error = Error;
            fn $method<V: Serialize + ?Sized>(&mut self, value: &V) -> Result<(), Error> {
                self.0.$method(value)
            }
            fn end(self) -> Result<Self::Ok, Error> {
                self.0.end().map(Some)
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);
impl<T: SerializeMap<Ok = Value, Error = Error>> SerializeMap for Finish<T> {
    type Ok = Option<Value>;
    type Error = Error;
    fn serialize_key<V: Serialize + ?Sized>(&mut self, key: &V) -> Result<(), Error> {
        self.0.serialize_key(key)
    }
    fn serialize_value<V: Serialize + ?Sized>(&mut self, value: &V) -> Result<(), Error> {
        self.0.serialize_value(value)
    }
    fn end(self) -> Result<Self::Ok, Error> {
        self.0.end().map(Some)
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl<T: $trait<Ok = Value, Error = Error>> $trait for Finish<T> {
            type Ok = Option<Value>;
            type Error = Error;
            fn serialize_field<V: Serialize + ?Sized>(
                &mut self,
                key: &'static str,
                value: &V,
            ) -> Result<(), Error> {
                self.0.serialize_field(key, value)
            }
            fn end(self) -> Result<Self::Ok, Error> {
                self.0.end().map(Some)
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);
