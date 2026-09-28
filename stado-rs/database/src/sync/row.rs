//! One row the fleet database answered, read by column name or position.

use std::fmt::Display;

use sea_orm::{ColIdx, QueryResult, TryGetable};
use serde_json::{Map, Value};

use super::{Error, Result};

pub struct Row<'a>(&'a QueryResult);

impl<'a> Row<'a> {
    pub(super) fn new(row: &'a QueryResult) -> Self {
        Self(row)
    }

    /// One column as `T`; a NULL reads as `None` when `T` is an `Option`.
    pub fn get<I, T>(&self, index: I) -> Result<T>
    where
        I: ColIdx + Display,
        T: TryGetable,
    {
        let column = index.to_string();
        self.0.try_get_by::<T, I>(index).map_err(|error| {
            Error::Conversion(format!("column {column} could not be read: {error}"))
        })
    }

    /// One column as the first JSON value its stored type decodes to.
    fn column_json(&self, column: &str) -> Result<Value> {
        if let Ok(value) = self.0.try_get_by::<Option<String>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<i64>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<i32>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<i16>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<f64>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<f32>, _>(column) {
            return Ok(Value::from(value));
        }
        if let Ok(value) = self.0.try_get_by::<Option<bool>, _>(column) {
            return Ok(Value::from(value));
        }
        Err(Error::Conversion(format!(
            "column {column} has a type that is not text, an integer, a float or a boolean"
        )))
    }

    /// Every column as a JSON object keyed by column name.
    pub fn json(&self) -> Result<Value> {
        let mut object = Map::new();
        for column in self.0.column_names() {
            let value = self.column_json(&column)?;
            object.insert(column, value);
        }
        Ok(Value::Object(object))
    }
}
