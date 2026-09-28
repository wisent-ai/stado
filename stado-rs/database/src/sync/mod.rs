//! The fleet database for a product whose code reads synchronously: a
//! command-line tool, or a service whose row operations are plain functions.
//!
//! `Client::connect` opens the same SeaORM connection as [`crate::connect`] on
//! a runtime of its own, and every statement is handed to that runtime and
//! waited for. A caller on a multi-threaded Tokio worker leaves the worker
//! for the wait (`block_in_place`); a caller on any other thread just waits.
//! So a product never keeps a Postgres client, a connector or a row mapper of
//! its own: `execute`, `query_row`, `prepare(..).query_map` and `transaction`
//! are here, with `params!` binding values of mixed types.

mod bind;
mod row;

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, QueryResult, SqlErr,
    Statement as SeaStatement, TransactionTrait,
};
use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

pub use bind::{Bind, NullOf, Params, Values};
pub use row::Row;

use crate::FleetDatabase;

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

/// Run `work` on `runtime` and wait for its answer; `None` if the task
/// stopped without one.
fn wait<T: Send + 'static>(runtime: &Runtime, work: impl Future<Output = T> + Send + 'static) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        let _ = sender.send(work.await);
    });
    let receive = move || receiver.recv().ok();
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => tokio::task::block_in_place(receive),
        _ => receive(),
    }
}

fn stopped() -> Error {
    Error::Conversion("the fleet database task stopped before it answered".to_owned())
}

fn statement(sql: &str, params: impl Params) -> SeaStatement {
    SeaStatement::from_sql_and_values(DbBackend::Postgres, sql, params.values())
}

/// The two places a statement runs: the connection, or a transaction on it.
pub trait Run {
    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>>;
    fn exec(&self, statement: SeaStatement) -> Result<u64>;
}

pub struct Client {
    runtime: Runtime,
    connection: DatabaseConnection,
}

impl Client {
    /// Resolve and open `database` as [`crate::connect`] does; a refusal names
    /// the step that failed.
    pub fn connect(database: &FleetDatabase) -> std::result::Result<Self, crate::Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("stado-database")
            .enable_all()
            .build()
            .map_err(|error| crate::Error::new("connect", format!("the database runtime could not start: {error}")))?;
        let fleet = database.clone();
        let connection = wait(&runtime, async move { crate::connect(&fleet).await })
            .ok_or_else(|| crate::Error::new("connect", "the connecting task stopped before it answered"))??;
        Ok(Self { runtime, connection })
    }

    /// Several statements without parameters, as a schema file holds them.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let connection = self.connection.clone();
        let sql = sql.to_owned();
        wait(&self.runtime, async move { connection.execute_unprepared(&sql).await.map(drop) })
            .ok_or_else(stopped)??;
        Ok(())
    }

    /// A transaction: committed by `commit`, rolled back when dropped without it.
    pub fn transaction(&self) -> Result<Tx<'_>> {
        let connection = self.connection.clone();
        let transaction = wait(&self.runtime, async move { connection.begin().await }).ok_or_else(stopped)??;
        Ok(Tx {
            client: self,
            transaction: Some(Arc::new(transaction)),
        })
    }
}

impl Run for Client {
    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        let connection = self.connection.clone();
        Ok(wait(&self.runtime, async move { connection.query_all(statement).await }).ok_or_else(stopped)??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let connection = self.connection.clone();
        let done = wait(&self.runtime, async move { connection.execute(statement).await }).ok_or_else(stopped)??;
        Ok(done.rows_affected())
    }
}

pub struct Tx<'c> {
    client: &'c Client,
    transaction: Option<Arc<DatabaseTransaction>>,
}

impl Tx<'_> {
    fn held(&self) -> Result<Arc<DatabaseTransaction>> {
        self.transaction
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| Error::Conversion("the transaction has already ended".to_owned()))
    }

    fn owned(&mut self) -> Option<DatabaseTransaction> {
        self.transaction.take().and_then(|held| Arc::try_unwrap(held).ok())
    }

    pub fn commit(mut self) -> Result<()> {
        let transaction = self
            .owned()
            .ok_or_else(|| Error::Conversion("the transaction is still in use".to_owned()))?;
        wait(&self.client.runtime, async move { transaction.commit().await }).ok_or_else(stopped)??;
        Ok(())
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if let Some(transaction) = self.owned() {
            let _ = wait(&self.client.runtime, async move { transaction.rollback().await });
        }
    }
}

impl Run for Tx<'_> {
    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        let transaction = self.held()?;
        Ok(wait(&self.client.runtime, async move { transaction.query_all(statement).await }).ok_or_else(stopped)??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let transaction = self.held()?;
        let done = wait(&self.client.runtime, async move { transaction.execute(statement).await })
            .ok_or_else(stopped)??;
        Ok(done.rows_affected())
    }
}

macro_rules! statements {
    ($runner:ty) => {
        impl $runner {
            /// Rows changed.
            pub fn execute(&self, sql: &str, params: impl Params) -> Result<u64> {
                self.exec(statement(sql, params))
            }

            /// The first row the statement answers, mapped; `Error::NoRows` if none.
            pub fn query_row<T, F>(&self, sql: &str, params: impl Params, map: F) -> Result<T>
            where
                F: FnOnce(&Row<'_>) -> Result<T>,
            {
                let rows = self.rows(statement(sql, params))?;
                let first = rows.first().ok_or(Error::NoRows)?;
                map(&Row::new(first))
            }

            pub fn prepare(&self, sql: &str) -> Result<Statement<'_, Self>> {
                Ok(Statement {
                    runner: self,
                    sql: sql.to_owned(),
                })
            }
        }
    };
}
statements!(Client);
statements!(Tx<'_>);

/// A statement answering many rows.
pub struct Statement<'r, R: ?Sized> {
    runner: &'r R,
    sql: String,
}

impl<R: Run> Statement<'_, R> {
    pub fn query_map<T, F>(&mut self, params: impl Params, mut map: F) -> Result<std::vec::IntoIter<Result<T>>>
    where
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        let rows = self.runner.rows(statement(&self.sql, params))?;
        let mapped: Vec<Result<T>> = rows.iter().map(|row| map(&Row::new(row))).collect();
        Ok(mapped.into_iter())
    }
}
