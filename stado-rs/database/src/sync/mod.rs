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
mod postgres;
mod row;

use std::future::Future;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, QueryResult,
    RuntimeErr, Statement as SeaStatement, TransactionTrait,
};
use sqlx::{PgPool, Postgres};
use tokio::runtime::{Handle, Runtime, RuntimeFlavor};
use tokio::sync::Mutex;

pub use bind::{Bind, NullOf, Params, Values};
pub use error::{Error, OptionalExtension, Result};
pub use row::Row;

use crate::FleetDatabase;

/// Run `work` on `runtime` and wait for its answer; `None` if the task
/// stopped without one.
fn wait<T: Send + 'static>(
    runtime: &Runtime,
    work: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        let _ = sender.send(work.await);
    });
    let receive = move || receiver.recv().ok();
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(receive)
        }
        _ => receive(),
    }
}

fn stopped() -> Error {
    Error::Conversion("the fleet database task stopped before it answered".to_owned())
}

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
        let connection =
            wait(&runtime, async move { crate::connect(&fleet).await }).ok_or_else(|| {
                crate::Error::new("connect", "the connecting task stopped before it answered")
            })??;
        Ok(Self {
            runtime,
            connection,
        })
    }

    /// Several statements without parameters, as a schema file holds them.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let connection = self.connection.clone();
        let sql = sql.to_owned();
        wait(&self.runtime, async move {
            connection.execute_unprepared(&sql).await.map(drop)
        })
        .ok_or_else(stopped)??;
        Ok(())
    }

    /// A transaction: committed by `commit`, rolled back when dropped without it.
    pub fn transaction(&self) -> Result<Tx<'_>> {
        let held = match self.postgres() {
            Some(pool) => {
                let transaction = wait(&self.runtime, async move { pool.begin().await })
                    .ok_or_else(stopped)?
                    .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(error)))?;
                Held::Postgres(Arc::new(Mutex::new(transaction)))
            }
            None => {
                let connection = self.connection.clone();
                let transaction = wait(&self.runtime, async move { connection.begin().await })
                    .ok_or_else(stopped)??;
                Held::Sea(Arc::new(transaction))
            }
        };
        Ok(Tx {
            client: self,
            transaction: Some(held),
        })
    }

    /// The Postgres pool under the connection, on which statements go out
    /// unnamed (see [`postgres`]); `None` on MySQL and SQLite.
    fn postgres(&self) -> Option<PgPool> {
        (self.connection.get_database_backend() == DbBackend::Postgres)
            .then(|| self.connection.get_postgres_connection_pool().clone())
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
        wait(&self.runtime, work(self.connection.clone())).ok_or_else(stopped)
    }
}

impl Run for Client {
    fn backend(&self) -> DbBackend {
        self.connection.get_database_backend()
    }

    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        if let Some(pool) = self.postgres() {
            return Ok(wait(&self.runtime, async move {
                postgres::rows_alone(&pool, statement).await
            })
            .ok_or_else(stopped)??);
        }
        let connection = self.connection.clone();
        Ok(wait(&self.runtime, async move {
            connection.query_all(statement).await
        })
        .ok_or_else(stopped)??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        if let Some(pool) = self.postgres() {
            return Ok(wait(&self.runtime, async move {
                postgres::exec_alone(&pool, statement).await
            })
            .ok_or_else(stopped)??);
        }
        let connection = self.connection.clone();
        let done = wait(
            &self.runtime,
            async move { connection.execute(statement).await },
        )
        .ok_or_else(stopped)??;
        Ok(done.rows_affected())
    }
}

/// An open transaction: on Postgres the pool's own, whose statements go out
/// unnamed; on MySQL and SQLite SeaORM's.
#[derive(Clone)]
enum Held {
    Postgres(Arc<Mutex<sqlx::Transaction<'static, Postgres>>>),
    Sea(Arc<DatabaseTransaction>),
}

/// A transaction no statement holds any more, ready to end.
enum Owned {
    Postgres(sqlx::Transaction<'static, Postgres>),
    Sea(DatabaseTransaction),
}

pub struct Tx<'c> {
    client: &'c Client,
    transaction: Option<Held>,
}

impl Tx<'_> {
    fn held(&self) -> Result<Held> {
        self.transaction
            .clone()
            .ok_or_else(|| Error::Conversion("the transaction has already ended".to_owned()))
    }

    /// Ends the transaction: commits it when `commit`, else rolls it back.
    /// `None` when a statement still holds it.
    fn finish(
        &mut self,
        commit: bool,
    ) -> Option<impl Future<Output = std::result::Result<(), DbErr>> + Send + 'static> {
        let owned = match self.transaction.take()? {
            Held::Postgres(shared) => Owned::Postgres(Arc::try_unwrap(shared).ok()?.into_inner()),
            Held::Sea(shared) => Owned::Sea(Arc::try_unwrap(shared).ok()?),
        };
        Some(async move {
            match owned {
                Owned::Postgres(transaction) => {
                    let ended = if commit {
                        transaction.commit().await
                    } else {
                        transaction.rollback().await
                    };
                    ended.map_err(|error| DbErr::Exec(RuntimeErr::SqlxError(error)))
                }
                Owned::Sea(transaction) => {
                    if commit {
                        transaction.commit().await
                    } else {
                        transaction.rollback().await
                    }
                }
            }
        })
    }

    pub fn commit(mut self) -> Result<()> {
        let ending = self
            .finish(true)
            .ok_or_else(|| Error::Conversion("the transaction is still in use".to_owned()))?;
        wait(&self.client.runtime, ending).ok_or_else(stopped)??;
        Ok(())
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if let Some(ending) = self.finish(false) {
            let _ = wait(&self.client.runtime, ending);
        }
    }
}

impl Run for Tx<'_> {
    fn backend(&self) -> DbBackend {
        self.client.connection.get_database_backend()
    }

    fn rows(&self, statement: SeaStatement) -> Result<Vec<QueryResult>> {
        let rows = match self.held()? {
            Held::Postgres(lock) => wait(&self.client.runtime, async move {
                let mut transaction = lock.lock().await;
                postgres::rows(&mut **transaction, statement).await
            }),
            Held::Sea(transaction) => wait(&self.client.runtime, async move {
                transaction.query_all(statement).await
            }),
        };
        Ok(rows.ok_or_else(stopped)??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let done = match self.held()? {
            Held::Postgres(lock) => wait(&self.client.runtime, async move {
                let mut transaction = lock.lock().await;
                postgres::exec(&mut **transaction, statement).await
            }),
            Held::Sea(transaction) => wait(&self.client.runtime, async move {
                transaction
                    .execute(statement)
                    .await
                    .map(|done| done.rows_affected())
            }),
        };
        Ok(done.ok_or_else(stopped)??)
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
