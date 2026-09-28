# stado-database

A Wisent product reaches its durable state through this crate and SeaORM. It
never opens a private SQLite file, never writes its own Postgres client, and
never holds a database address or password of its own.

```toml
[dependencies]
stado-database = { git = "https://github.com/wisent-ai/stado", package = "stado-database" }
sea-orm = { version = "1.1", default-features = false, features = ["sqlx-postgres", "runtime-tokio-rustls", "macros"] }
```

```rust
let database = stado_database::FleetDatabase::for_product("handtohuman", "HANDTOHUMAN_FLEET_HOME")?;
let connection: sea_orm::DatabaseConnection = stado_database::connect(&database).await?;
```

Tables are SeaORM entities in the product; the schema changes through a
`sea-orm-migration` migrator the product runs at startup.

## Synchronous code

A command-line tool, or a service whose row operations are plain functions,
uses `stado_database::sync::Client` instead of writing a client of its own:

```rust
use stado_database::{params, sync::{Client, OptionalExtension}};

let client = Client::connect(&stado_database::FleetDatabase::for_product("grant-cli", "GRANT_FLEET_HOME")?)?;
client.execute("UPDATE sources SET last_synced_at = $1 WHERE id = $2", params![now, id])?;
let name: Option<String> = client
    .query_row("SELECT name FROM sources WHERE id = $1", [id], |row| row.get("name"))
    .optional()?;
let transaction = client.transaction()?;
transaction.execute("DELETE FROM watches WHERE id = $1", [id])?;
transaction.commit()?; // dropped without commit, it rolls back
```

The client runs the SeaORM connection on a runtime of its own and waits for
each statement; on a multi-threaded Tokio worker it leaves the worker for the
wait. `Error::is_unique_violation` says an insert hit a unique constraint, and
`Row::json` gives a whole row as a JSON object.

A product whose tables are SeaORM entities but whose callers are synchronous
hands its entity work to the same client with `run`, and runs its migrator
the same way; it keeps no runtime or connection thread of its own:

```rust
let client = Client::connect(&FleetDatabase::for_product("jeden", "JEDEN_STADO_HOME")?)?;
client.run(|db| async move { Migrator::up(&db, None).await })??;
let row = client.run(move |db| async move { value::Entity::find_by_id(key).one(&db).await })??;
```

`run` answers the closure's own result inside `sync::Result`; its only error
of its own is a task that stopped before answering.

## What `connect` does

1. `stado database resolve <product> --consumer <product> --json` names the
   Skarbiec item holding the database (`<product>-database`).
2. `stado service directory connect skarbiec --consumer <product> --json`
   names the Skarbiec route on this host.
3. `stado secrets get <item> --field pooler_url` and `--field ca_certificate`,
   run as consumer `<product>-database-client` with the bearer in
   `~/.stado/<product>-database-client-skarbiec-token`, give the pooler URL
   and the provider's root certificate.
4. The pool connects with `sslmode=verify-full` against that certificate.

## Provisioning a product once

```sh
stado database create <product> --consumer <product> --accept-monthly-usd <usd>
stado credentials token mint <product>-database-client --host <vault owner> \
  --capabilities 'read:<product>-database#pooler_url,read:<product>-database#ca_certificate' \
  --audience <product>-database-client --token-file-name <product>-database-client-skarbiec-token
stado credentials token sync <product>-database-client --from-host <vault owner> --host <this host> \
  --source-token-file <owner home>/.stado/<product>-database-client-skarbiec-token \
  --token-file ~/.stado/<product>-database-client-skarbiec-token --shared-vault
stado service directory consumer-add skarbiec <product> --target <this host> --bind 127.0.0.1:<port>
```

## Refusals

Every failure is `the fleet database could not be reached at step <step>:
<detail>`, where the step is one of `locate Stado`, `resolve database`,
`resolve Skarbiec route`, `read credential field`, `read pooler_url` or
`connect`, and the detail carries Stado's own answer. For example an
undeclared database reads `step resolve database: stado database resolve
<product> ... exited 2: Error: unknown database "<product>"; declared: ...`,
and a route the resolver has not bound yet reads `step resolve Skarbiec route:
... did not answer at http://127.0.0.1:<port>`.
