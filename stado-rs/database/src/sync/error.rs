//! What a synchronous fleet-database call answers when it fails, and the
//! `optional` reading of a statement whose row may be absent.

use std::fmt;

use sea_orm::{DbErr, SqlErr};

#[derive(Debug)]
pub enum Error {
    /// A statement that must answer one row answered none.
    NoRows,
    Database(DbErr),
    /// A stored value does not fit the Rust type it is read as.
    Conversion(String),
}

impl Error {
    /// The statement hit a unique constraint: the row already exists.
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, Self::Database(error) if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRows => formatter.write_str("the fleet database answered no row"),
            Self::Database(error) => write!(formatter, "the fleet database refused: {error}"),
            Self::Conversion(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbErr> for Error {
    fn from(error: DbErr) -> Self {
        Self::Database(error)
    }
}

impl From<std::num::TryFromIntError> for Error {
    fn from(error: std::num::TryFromIntError) -> Self {
        Self::Conversion(format!("a stored integer does not fit: {error}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Turns "no row" into `None` for statements whose row may be absent.
pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
