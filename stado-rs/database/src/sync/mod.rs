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
mod waiting;

use std::future::Future;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, QueryResult,
    Statement as SeaStatement, TransactionTrait,
};
use tokio::runtime::Runtime;

pub use bind::{Bind, NullOf, Params, Values};
pub use error::{Error, OptionalExtension, Result};
pub use row::Row;

use crate::FleetDatabase;
use waiting::{answer, stopped, wait, STOPPED};

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
    runtime: Runtime,
    connection: DatabaseConnection,
    /// The database's Stado name, where each statement's wait says it waits.
    name: String,
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
            .map_err(|error| {
                crate::Error::new(
                    "connect",
                    format!("the database runtime could not start: {error}"),
                )
            })?;
        let fleet = database.clone();
        let connection = answer(&runtime, async move { crate::connect(&fleet).await })
            .ok_or_else(|| {
                crate::Error::new("connect", "the connecting task stopped before it answered")
            })??;
        Ok(Self {
            runtime,
            connection,
            name: database.name.clone(),
        })
    }

    /// Several statements without parameters, as a schema file holds them.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let connection = self.connection.clone();
        let what = format!("execute a batch of {} bytes of SQL", sql.len());
        let sql = sql.to_owned();
        wait(&self.runtime, what, &self.name, async move {
            connection.execute_unprepared(&sql).await.map(drop)
        })
        .ok_or_else(stopped)??;
        Ok(())
    }

    /// A transaction: committed by `commit`, rolled back when dropped without it.
    pub fn transaction(&self) -> Result<Tx<'_>> {
        let connection = self.connection.clone();
        let transaction = wait(
            &self.runtime,
            "begin a transaction",
            &self.name,
            async move { connection.begin().await },
        )
        .ok_or_else(stopped)??;
        Ok(Tx {
            client: self,
            transaction: Some(Arc::new(transaction)),
        })
    }

    /// Run SeaORM work — entity queries, a transaction, a migrator — on the
    /// connection and wait for its answer, for a product whose tables are
    /// SeaORM entities but whose callers are synchronous. `work` answers the
    /// product's own result; only a stopped task is this crate's error.
    pub fn run<T, F>(&self, work: impl FnOnce(DatabaseConnection) -> F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let waiting = stado_wait::begin(stado_wait::Kind::Database, "SeaORM work", &self.name);
        match answer(&self.runtime, work(self.connection.clone())) {
            Some(answered) => {
                waiting.done();
                Ok(answered)
            }
            None => {
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
        Ok(wait(&self.runtime, what, &self.name, async move {
            connection.query_all(statement).await
        })
        .ok_or_else(stopped)??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let connection = self.connection.clone();
        let what = statement.sql.clone();
        let done = wait(&self.runtime, what, &self.name, async move {
            connection.execute(statement).await
        })
        .ok_or_else(stopped)??;
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
            .clone()
            .ok_or_else(|| Error::Conversion("the transaction has already ended".to_owned()))
    }

    /// Ends the transaction: commits it when `commit`, else rolls it back.
    /// `None` when a statement still holds it.
    fn finish(
        &mut self,
        commit: bool,
    ) -> Option<impl Future<Output = std::result::Result<(), sea_orm::DbErr>> + Send + 'static>
    {
        let transaction = Arc::try_unwrap(self.transaction.take()?).ok()?;
        Some(async move {
            if commit {
                transaction.commit().await
            } else {
                transaction.rollback().await
            }
        })
    }

    pub fn commit(mut self) -> Result<()> {
        let ending = self
            .finish(true)
            .ok_or_else(|| Error::Conversion("the transaction is still in use".to_owned()))?;
        let client = self.client;
        wait(
            &client.runtime,
            "commit the transaction",
            &client.name,
            ending,
        )
        .ok_or_else(stopped)??;
        Ok(())
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if let Some(ending) = self.finish(false) {
            let client = self.client;
            let _ = wait(
                &client.runtime,
                "roll the transaction back",
                &client.name,
                ending,
            );
        }
    }
}

impl Run for Tx<'_> {
    fn backend(&self) -> DbBackend {
        self.client.connection.get_database_backend()
    }

    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        let transaction = self.held()?;
        let what = statement.sql.clone();
        Ok(
            wait(&self.client.runtime, what, &self.client.name, async move {
                transaction.query_all(statement).await
            })
            .ok_or_else(stopped)??,
        )
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let transaction = self.held()?;
        let what = statement.sql.clone();
        Ok(
            wait(&self.client.runtime, what, &self.client.name, async move {
                transaction
                    .execute(statement)
                    .await
                    .map(|done| done.rows_affected())
            })
            .ok_or_else(stopped)??,
        )
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
