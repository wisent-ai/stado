//! One Rust value as a statement parameter. A NULL keeps the column type of
//! the value it stands for, so `$2::text IS NULL` and a typed insert read it
//! the same way a present value would.

use sea_orm::Value;

pub trait Bind {
    fn bind(&self) -> Value;

    /// The NULL of this type; text unless the type says otherwise.
    fn null() -> Value
    where
        Self: Sized,
    {
        Value::String(None)
    }
}

/// The NULL of a type that may be unsized (`str`), for `&T`.
pub trait NullOf {
    fn null_of() -> Value;
}

impl<T: Bind> NullOf for T {
    fn null_of() -> Value {
        T::null()
    }
}

impl NullOf for str {
    fn null_of() -> Value {
        Value::String(None)
    }
}

impl<T: Bind + NullOf + ?Sized> Bind for &T {
    fn bind(&self) -> Value {
        (**self).bind()
    }

    fn null() -> Value {
        T::null_of()
    }
}

impl<T: Bind> Bind for Option<T> {
    fn bind(&self) -> Value {
        match self {
            Some(value) => value.bind(),
            None => T::null(),
        }
    }

    fn null() -> Value {
        T::null()
    }
}

impl Bind for str {
    fn bind(&self) -> Value {
        Value::String(Some(Box::new(self.to_owned())))
    }
}

impl Bind for String {
    fn bind(&self) -> Value {
        Value::String(Some(Box::new(self.clone())))
    }
}

/// A `TEXT[]` parameter.
impl Bind for Vec<String> {
    fn bind(&self) -> Value {
        let items = self.iter().map(|item| item.bind()).collect();
        Value::Array(sea_orm::sea_query::ArrayType::String, Some(Box::new(items)))
    }

    fn null() -> Value {
        Value::Array(sea_orm::sea_query::ArrayType::String, None)
    }
}

macro_rules! scalar {
    ($($rust:ty => $variant:ident),* $(,)?) => {
        $(impl Bind for $rust {
            fn bind(&self) -> Value {
                Value::$variant(Some(*self))
            }

            fn null() -> Value {
                Value::$variant(None)
            }
        })*
    };
}

scalar!(i64 => BigInt, i32 => Int, i16 => SmallInt, f64 => Double, f32 => Float, bool => Bool);

/// Parameters of mixed types, built by [`params!`](crate::params).
pub struct Values(pub Vec<Value>);

/// What a statement can be bound with: `params![...]` or an array of one type.
pub trait Params {
    fn values(&self) -> Vec<Value>;
}

impl Params for Values {
    fn values(&self) -> Vec<Value> {
        self.0.clone()
    }
}

impl<T: Bind, const N: usize> Params for [T; N] {
    fn values(&self) -> Vec<Value> {
        self.iter().map(Bind::bind).collect()
    }
}

/// Binds values of mixed types: `params![id, name, now()]`.
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        $crate::sync::Values(vec![$($crate::sync::Bind::bind(&$value)),*])
    };
}
