use std::cmp;

use crate::{
    binary::{Encoder, ReadEx},
    errors::Error,
    types::{from_sql::*, Column, SqlType, Value, ValueRef, column::column_data::BoxColumnData},
};

use super::column_data::ColumnData;

pub(crate) struct FixedStringColumnData {
    buffer: Vec<u8>,
    str_len: usize,
}

pub(crate) struct FixedStringAdapter {
    pub(crate) column: Column,
    pub(crate) str_len: usize,
}

pub(crate) struct NullableFixedStringAdapter {
    pub(crate) column: Column,
    pub(crate) str_len: usize,
}

impl FixedStringColumnData {
    pub fn with_capacity(capacity: usize, str_len: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity * str_len),
            str_len,
        }
    }

    pub(crate) fn load<T: ReadEx>(
        reader: &mut T,
        size: usize,
        str_len: usize,
    ) -> Result<Self, Error> {
        let mut instance = Self::with_capacity(size, str_len);

        for _ in 0..size {
            let old_len = instance.buffer.len();
            instance.buffer.resize(old_len + str_len, 0_u8);
            reader.read_bytes(&mut instance.buffer[old_len..old_len + str_len])?;
        }

        Ok(instance)
    }
}

impl ColumnData for FixedStringColumnData {
    fn sql_type(&self) -> SqlType {
        SqlType::FixedString(self.str_len)
    }

    fn save(&self, encoder: &mut Encoder, start: usize, end: usize) {
        let start_index = start * self.str_len;
        let end_index = end * self.str_len;
        encoder.write_bytes(&self.buffer[start_index..end_index]);
    }

    fn len(&self) -> usize {
        self.buffer.len() / self.str_len
    }

    fn push(&mut self, value: Value) {
        let bs: String = String::from(value);
        let l = cmp::min(bs.len(), self.str_len);
        let old_len = self.buffer.len();
        self.buffer.extend_from_slice(&bs.as_bytes()[0..l]);
        self.buffer.resize(old_len + (self.str_len - l), 0_u8);
    }

    fn at(&self, index: usize) -> ValueRef {
        let shift = index * self.str_len;
        let str_ref = &self.buffer[shift..shift + self.str_len];
        ValueRef::String(str_ref)
    }

    fn clone_instance(&self) -> BoxColumnData {
        Box::new(Self {
            buffer: self.buffer.clone(),
            str_len: self.str_len,
        })
    }
}

impl ColumnData for FixedStringAdapter {
    fn sql_type(&self) -> SqlType {
        SqlType::FixedString(self.str_len)
    }

    fn save(&self, encoder: &mut Encoder, start: usize, end: usize) {
        let mut buffer = Vec::with_capacity(self.str_len);
        for index in start..end {
            buffer.resize(0, 0);
            match self.column.at(index) {
                ValueRef::String(_) => {
                    let string_ref = self.column.at(index).as_bytes().unwrap();
                    buffer.extend(string_ref.as_ref());
                }
                ValueRef::Array(SqlType::UInt8, vs) => {
                    let mut string_val: Vec<u8> = Vec::with_capacity(vs.len());
                    for v in vs.iter() {
                        let byte: u8 = v.clone().into();
                        string_val.push(byte);
                    }
                    let string_ref: &[u8] = string_val.as_ref();
                    buffer.extend(string_ref);
                }
                _ => unimplemented!(),
            }
            buffer.resize(self.str_len, 0);
            encoder.write_bytes(&buffer[..]);
        }
    }

    fn len(&self) -> usize {
        self.column.len()
    }

    fn push(&mut self, _value: Value) {
        unimplemented!()
    }

    fn at(&self, index: usize) -> ValueRef {
        self.column.at(index)
    }

    fn clone_instance(&self) -> BoxColumnData {
        unimplemented!()
    }
}

impl ColumnData for NullableFixedStringAdapter {
    fn sql_type(&self) -> SqlType {
        SqlType::Nullable(SqlType::FixedString(self.str_len).into())
    }

    fn save(&self, encoder: &mut Encoder, start: usize, end: usize) {
        let size = end - start;
        let mut nulls = vec![0; size];
        let mut values: Vec<Option<&[u8]>> = vec![None; size];

        for (i, index) in (start..end).enumerate() {
            values[i] = Option::from_sql(self.at(index)).unwrap();
            if values[i].is_none() {
                nulls[i] = 1;
            }
        }

        encoder.write_bytes(nulls.as_ref());

        let mut buffer = Vec::with_capacity(self.str_len);
        for value in values {
            buffer.resize(0, 0);
            if let Some(string_ref) = value {
                buffer.extend(string_ref);
            }
            buffer.resize(self.str_len, 0);
            encoder.write_bytes(buffer.as_ref());
        }
    }

    fn len(&self) -> usize {
        self.column.len()
    }

    fn push(&mut self, _value: Value) {
        unimplemented!()
    }

    fn at(&self, index: usize) -> ValueRef {
        self.column.at(index)
    }

    fn clone_instance(&self) -> BoxColumnData {
        unimplemented!()
    }
}
