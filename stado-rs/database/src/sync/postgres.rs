//! Postgres statements sent unnamed.
//!
//! The pooler Stado hands out runs in transaction mode: a server connection
//! serves another client after every transaction. sqlx names every
//! persistent statement (`sqlx_s_1`, `sqlx_s_2`, …) whatever its statement
//! cache holds, and SeaORM sends every statement persistent, so a named
//! statement stays on the server connection and the next client that
//! prepares the same name is refused `prepared statement "sqlx_s_1" already
//! exists`. Here each statement goes out unnamed (`persistent(false)`), which
//! Postgres replaces at the next parse, so nothing is left behind on a pooled
//! server connection. sqlx parses an unnamed statement in one round trip and
//! binds it in the next; between them, outside a transaction, the pooler may
//! move the client to another server connection, where the statement is
//! `unnamed prepared statement does not exist`. So a statement that runs on
//! its own runs inside a transaction of its own, which the pooler keeps on
//! one server connection.

use sea_orm::sea_query::Values;
use sea_orm::{DbErr, QueryResult, RuntimeErr, Statement as SeaStatement};
use sea_query_binder::SqlxValues;
use sqlx::{Executor, PgPool, Postgres};

fn unnamed(statement: &SeaStatement) -> sqlx::query::Query<'_, Postgres, SqlxValues> {
    let values = statement.values.clone().unwrap_or(Values(Vec::new()));
    sqlx::query_with(&statement.sql, SqlxValues(values)).persistent(false)
}

/// The rows `statement` answers.
pub(super) async fn rows<'c, E>(
    executor: E,
    statement: SeaStatement,
) -> Result<Vec<QueryResult>, DbErr>
where
    E: Executor<'c, Database = Postgres>,
{
    unnamed(&statement)
        .fetch_all(executor)
        .await
        .map(|rows| rows.into_iter().map(QueryResult::from).collect())
        .map_err(|error| DbErr::Query(RuntimeErr::SqlxError(error)))
}

/// The number of rows `statement` changed.
pub(super) async fn exec<'c, E>(executor: E, statement: SeaStatement) -> Result<u64, DbErr>
where
    E: Executor<'c, Database = Postgres>,
{
    unnamed(&statement)
        .execute(executor)
        .await
        .map(|done| done.rows_affected())
        .map_err(|error| DbErr::Exec(RuntimeErr::SqlxError(error)))
}

/// The rows `statement` answers, run in a transaction of its own.
pub(super) async fn rows_alone(
    pool: &PgPool,
    statement: SeaStatement,
) -> Result<Vec<QueryResult>, DbErr> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(error)))?;
    let answered = rows(&mut *transaction, statement).await?;
    transaction
        .commit()
        .await
        .map_err(|error| DbErr::Exec(RuntimeErr::SqlxError(error)))?;
    Ok(answered)
}

/// The number of rows `statement` changed, run in a transaction of its own.
pub(super) async fn exec_alone(pool: &PgPool, statement: SeaStatement) -> Result<u64, DbErr> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(error)))?;
    let changed = exec(&mut *transaction, statement).await?;
    transaction
        .commit()
        .await
        .map_err(|error| DbErr::Exec(RuntimeErr::SqlxError(error)))?;
    Ok(changed)
}
