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
mod error;
mod row;
mod transaction;
mod waiting;

use std::future::Future;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement as SeaStatement,
    TransactionTrait,
};
use tokio::runtime::Runtime;

pub use bind::{Bind, NullOf, Params, Values};
pub use error::{Error, OptionalExtension, Result};
pub use row::Row;
pub use transaction::Tx;

use crate::FleetDatabase;
use stado_wait::Cancellation;
use waiting::{answer, stopped, wait, Answer, STOPPED};

/// The statement in the dialect of the database it runs on: Postgres, MySQL
/// or SQLite, as the connection's backend says.
fn statement(backend: DbBackend, sql: &str, params: impl Params) -> SeaStatement {
    SeaStatement::from_sql_and_values(backend, sql, params.values())
}

/// The two places a statement runs: the connection, or a transaction on it.
pub trait Run {
    fn backend(&self) -> DbBackend;
    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>>;
    fn exec(&self, statement: SeaStatement) -> Result<u64>;
}

pub struct Client {
    pub(super) runtime: Runtime,
    pub(super) connection: DatabaseConnection,
    /// The database's Stado name, where each statement's wait says it waits.
    pub(super) name: String,
}

impl Client {
    /// Resolve and open `database` as [`crate::connect`] does; a refusal names
    /// the step that failed.
    pub fn connect(database: &FleetDatabase) -> std::result::Result<Self, crate::Error> {
        Self::open(database, None)
    }

    /// Cancel connection establishment without attaching that request's
    /// lifetime to the returned shared client. Later operations choose their
    /// own cancellation through `run_cancellable`.
    pub fn connect_cancellable(
        database: &FleetDatabase,
        cancellation: &Cancellation,
    ) -> std::result::Result<Self, crate::Error> {
        Self::open(database, Some(cancellation.clone()))
    }

    fn open(
        database: &FleetDatabase,
        cancellation: Option<Cancellation>,
    ) -> std::result::Result<Self, crate::Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("stado-database")
            .enable_all()
            .build()
            .map_err(|error| {
                crate::Error::new(
                    "connect",
                    format!("the database runtime could not start: {error}"),
                )
            })?;
        let fleet = database.clone();
        // The resolver stages take the cancellation themselves (each child
        // process is killed and reaped on it); the outer hand-off is not
        // raced, so no stage's cleanup is dropped half way.
        let connection = match answer(&runtime, None, async move {
            match cancellation {
                Some(cancellation) => crate::connect_cancellable(&fleet, &cancellation).await,
                None => crate::connect(&fleet).await,
            }
        }) {
            Answer::Answered(connection) => connection?,
            Answer::Cancelled => {
                return Err(crate::Error::new("connect", stado_wait::CANCELLED));
            }
            Answer::Stopped => {
                return Err(crate::Error::new(
                    "connect",
                    "the connecting task stopped before it answered",
                ));
            }
        };
        Ok(Self {
            runtime,
            connection,
            name: database.name.clone(),
        })
    }

    /// One statement or step on this client's runtime.
    pub(super) fn wait<U, E>(
        &self,
        what: impl std::fmt::Display,
        work: impl Future<Output = std::result::Result<U, E>> + Send + 'static,
    ) -> Result<std::result::Result<U, E>>
    where
        U: Send + 'static,
        E: std::error::Error + Send + 'static,
    {
        wait(&self.runtime, None, what, &self.name, work)
    }

    /// Several statements without parameters, as a schema file holds them.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let connection = self.connection.clone();
        let what = format!("execute a batch of {} bytes of SQL", sql.len());
        let sql = sql.to_owned();
        self.wait(what, async move {
            connection.execute_unprepared(&sql).await.map(drop)
        })??;
        Ok(())
    }

    /// A transaction: committed by `commit`, rolled back when dropped without it.
    pub fn transaction(&self) -> Result<Tx<'_>> {
        let connection = self.connection.clone();
        let transaction =
            self.wait(
                "begin a transaction",
                async move { connection.begin().await },
            )??;
        Ok(Tx::new(self, Arc::new(transaction)))
    }

    /// Run SeaORM work — entity queries, a transaction, a migrator — on the
    /// connection and wait for its answer, for a product whose tables are
    /// SeaORM entities but whose callers are synchronous. `work` answers the
    /// product's own result; a stopped task is this crate's error.
    pub fn run<T, F>(&self, work: impl FnOnce(DatabaseConnection) -> F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        self.run_under(None, work)
    }

    /// Run only this operation under the caller's cancellation. Cancelling
    /// it does not poison the shared client for subsequent operations.
    pub fn run_cancellable<T, F>(
        &self,
        cancellation: &Cancellation,
        work: impl FnOnce(DatabaseConnection) -> F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        self.run_under(Some(cancellation), work)
    }

    fn run_under<T, F>(
        &self,
        cancellation: Option<&Cancellation>,
        work: impl FnOnce(DatabaseConnection) -> F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let waiting = stado_wait::begin(stado_wait::Kind::Database, "SeaORM work", &self.name);
        match answer(&self.runtime, cancellation, work(self.connection.clone())) {
            Answer::Answered(answered) => {
                waiting.done();
                Ok(answered)
            }
            Answer::Cancelled => {
                waiting.failed(stado_wait::CANCELLED);
                Err(waiting::cancelled())
            }
            Answer::Stopped => {
                waiting.failed(STOPPED);
                Err(stopped())
            }
        }
    }
}

impl Run for Client {
    fn backend(&self) -> DbBackend {
        self.connection.get_database_backend()
    }

    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        let connection = self.connection.clone();
        let what = statement.sql.clone();
        Ok(self.wait(what, async move { connection.query_all(statement).await })??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let connection = self.connection.clone();
        let what = statement.sql.clone();
        let done = self.wait(what, async move { connection.execute(statement).await })??;
        Ok(done.rows_affected())
    }
}

macro_rules! statements {
    ($runner:ty) => {
        impl $runner {
            /// Rows changed.
            pub fn execute(&self, sql: &str, params: impl Params) -> Result<u64> {
                self.exec(statement(self.backend(), sql, params))
            }

            /// The first row the statement answers, mapped; `Error::NoRows` if none.
            pub fn query_row<T, F>(&self, sql: &str, params: impl Params, map: F) -> Result<T>
            where
                F: FnOnce(&Row<'_>) -> Result<T>,
            {
                let rows = self.rows(statement(self.backend(), sql, params))?;
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
    pub fn query_map<T, F>(
        &mut self,
        params: impl Params,
        mut map: F,
    ) -> Result<std::vec::IntoIter<Result<T>>>
    where
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        let rows = self
            .runner
            .rows(statement(self.runner.backend(), &self.sql, params))?;
        let mapped: Vec<Result<T>> = rows.iter().map(|row| map(&Row::new(row))).collect();
        Ok(mapped.into_iter())
    }
}
