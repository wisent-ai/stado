mod record;

use record::{require, required, Record, Result};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::json;
use stado_database::{notifications, FleetDatabase};

fn statement(sql: impl Into<String>) -> Statement {
    Statement::from_string(DbBackend::Postgres, sql.into())
}

async fn cases(connection: &DatabaseConnection, record: &mut Record) -> Result<()> {
    let channel = record.identity.clone();
    let mut changes = notifications::subscribe(connection, channel.clone()).await?;
    let table = format!("\"{}\"", record.identity);
    let before = connection
        .query_all(statement(format!("SELECT payload FROM {table}")))
        .await?;
    require(
        before.is_empty(),
        "the new qualification table was not empty",
    )?;
    record.observe(
        "subscribed-before-snapshot",
        json!({"channel": channel, "rows": []}),
    )?;

    let transaction = connection.begin().await?;
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("INSERT INTO {table} (payload) VALUES ($1)"),
            [channel.clone().into()],
        ))
        .await?;
    transaction
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_notify($1, $2)",
            [channel.clone().into(), channel.clone().into()],
        ))
        .await?;
    transaction.commit().await?;
    let wakeup = changes.next().await?;
    require(
        wakeup.channel() == channel && wakeup.payload() == channel,
        format!(
            "unexpected notification: channel {:?}, payload {:?}",
            wakeup.channel(),
            wakeup.payload()
        ),
    )?;
    let rows = connection
        .query_all(statement(format!("SELECT payload FROM {table}")))
        .await?;
    let persisted = rows
        .iter()
        .map(|row| row.try_get::<String>("", "payload"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    require(
        persisted == vec![channel.clone()],
        "notification did not correspond to the committed row",
    )?;
    record.observe(
        "committed-row-after-wakeup",
        json!({"channel": wakeup.channel(), "payload": wakeup.payload(), "rows": persisted}),
    )?;
    drop(changes);

    let declaration = connection
        .query_one(statement(
            "SELECT current_setting('max_identifier_length')::bigint AS maximum_bytes",
        ))
        .await?
        .ok_or("PostgreSQL returned no channel limit")?;
    let maximum: i64 = declaration.try_get("", "maximum_bytes")?;
    let mut overlong = "x".repeat(usize::try_from(maximum)?);
    overlong.push('x');
    for (name, channel) in [
        ("empty-channel", String::new()),
        ("nul-channel", "bad\0channel".to_owned()),
        ("overlong-channel", overlong),
    ] {
        let error = match notifications::subscribe(connection, channel).await {
            Ok(_) => {
                return Err(format!("{name}: invalid notification channel was accepted").into())
            }
            Err(error) => error,
        };
        require(
            error.step == "subscribe" && error.detail.contains("channel"),
            format!("{name}: unrelated refusal: {error}"),
        )?;
        record.observe(name, json!({"step": error.step, "cause": error.detail}))?;
    }
    Ok(())
}

async fn journey(record: &mut Record) -> Result<()> {
    let name = required("STADO_TEST_NOTIFICATION_DATABASE")?;
    let database = FleetDatabase::for_product(&name, "STADO_TEST_NOTIFICATION_FLEET_HOME")?;
    let connection = stado_database::connect(&database).await?;
    require(
        connection.get_database_backend() == DbBackend::Postgres,
        "notification qualification requires an isolated PostgreSQL database declaration",
    )?;
    let table = format!("\"{}\"", record.identity);
    connection
        .execute(statement(format!(
            "CREATE TABLE {table} (payload text NOT NULL)"
        )))
        .await?;
    record.observe(
        "created-isolated-table",
        json!({"database": name, "table": record.identity}),
    )?;
    let result = cases(&connection, record).await;
    // Cleanup is attempted even when a case fails, without replacing its cause.
    let cleanup = connection
        .execute(statement(format!("DROP TABLE {table}")))
        .await;
    record.observe(
        "drop-isolated-table",
        match &cleanup {
            Ok(_) => json!({"table": record.identity, "dropped": true}),
            Err(cause) => json!({"table": record.identity, "error": cause.to_string()}),
        },
    )?;
    result?;
    cleanup?;
    let absent = connection
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_regclass($1)::text AS relation",
            [record.identity.clone().into()],
        ))
        .await?
        .ok_or("PostgreSQL returned no table absence result")?;
    require(
        absent.try_get::<Option<String>>("", "relation")?.is_none(),
        "qualification table remains after cleanup",
    )?;
    record.observe("confirmed-table-absent", json!({"table": record.identity}))?;

    // Closing this test's own pool exercises a real terminal receive failure,
    // without stopping a shared database or another product's connection.
    let mut changes = notifications::subscribe(&connection, record.identity.clone()).await?;
    let pool = connection.get_postgres_connection_pool().clone();
    let closing = tokio::spawn(async move {
        pool.close().await;
    });
    let first = match changes.next().await {
        Ok(_) => return Err("closed pool produced a notification instead of a refusal".into()),
        Err(error) => error,
    };
    closing.await?;
    require(
        first.step == "receive notification" && first.detail.contains(&record.identity),
        format!("pool closure lost its operation or channel: {first}"),
    )?;
    require(
        changes.is_ended(),
        "the failed receive left its subscription active",
    )?;
    let second = match changes.next().await {
        Ok(_) => return Err("an ended subscription resumed receiving".into()),
        Err(error) => error,
    };
    require(
        changes.is_ended() && second.step == "receive notification",
        format!("ended handle lost its terminal state: {second}"),
    )?;
    record.observe(
        "pool-close-ends-subscription",
        json!({"first": first.to_string(), "next": second.to_string()}),
    )?;
    Ok(())
}

#[test]
fn real_notification_delivery_and_refusals() -> Result<()> {
    let mut record = Record::new()?;
    let result = (|| {
        record.provenance()?;
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(journey(&mut record))
    })();
    record.finish(&result)?;
    eprintln!(
        "notification journey evidence: {}",
        record.directory.display()
    );
    result
}
