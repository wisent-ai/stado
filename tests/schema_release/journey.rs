//! Real CLI, real SQLite files and an isolated PostgreSQL server. No fleet,
//! operator database, provider response or compiler is simulated here.
mod fixture;
#[cfg(unix)]
mod postgres;

use fixture::Run;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

const CREATE: &str = "create table schema_records (id integer primary key, value text not null);\n";
const INSERT: &str = "insert into schema_records values (7, 'archived');\n";
const BROKEN: &str =
    "create table must_rollback (id integer); select * from absent_migration_table;\n";

fn seed(run: &Run) {
    fs::write(run.source.join("migrations/001_create.sql"), CREATE).unwrap();
    fs::write(run.source.join("migrations/002_insert.sql"), INSERT).unwrap();
}

fn verify(run: &Run, engine: &str) -> Command {
    let mut command = run.stado();
    command.args(["product", "schema", "verify", "--engine", engine]);
    command
}

fn deliver(run: &Run, archive: &Path, digest: &str, url: &str) -> Command {
    let mut command = run.stado();
    command
        .args(["product", "deliver", "schema", "--engine", "postgres"])
        .env("WISENT_RELEASE_ARCHIVE", archive)
        .env("WISENT_RELEASE_SHA256", digest)
        .env(
            "WISENT_RELEASE_URI",
            format!("file://{}", archive.display()),
        )
        .env("SCHEMA_DATABASE_URL", url);
    command
}

fn sqlite(run: &mut Run, sql: &str) -> String {
    let mut command = run.command("sqlite3");
    command
        .arg("-readonly")
        .arg(run.output.join("schema-verify/scratch.sqlite"))
        .arg(sql);
    run.success(command).trim().to_owned()
}

#[test]
fn sqlite_verifies_built_bytes_and_rolls_back_a_failed_migration() {
    let mut run = Run::new();
    seed(&run);
    run.bundle();
    fs::write(
        run.source.join("migrations/001_create.sql"),
        "not the built migration;",
    )
    .unwrap();
    let command = verify(&run, "sqlite");
    let report: Value = serde_json::from_str(&run.success(command)).unwrap();
    assert_eq!(
        report["applied"],
        serde_json::json!(["001_create", "002_insert"])
    );
    assert_eq!(
        sqlite(&mut run, "select id || ':' || value from schema_records;"),
        "7:archived"
    );

    seed(&run);
    fs::write(run.source.join("migrations/003_broken.sql"), BROKEN).unwrap();
    run.bundle();
    let command = verify(&run, "sqlite");
    run.refuse(command, "003_broken.sql");
    assert_eq!(
        sqlite(&mut run, "select value from schema_records where id=7;"),
        "archived"
    );
    assert_eq!(
        sqlite(
            &mut run,
            "select count(*) from sqlite_master where name='must_rollback';"
        ),
        "0"
    );

    let command = verify(&run, "mysql");
    run.refuse(command, "mysql");
    let mut command = run.stado();
    command.args(["product", "schema", "verify"]);
    run.refuse(command, "--engine");
    let before = fs::read(run.output.join("schema-verify/scratch.sqlite")).unwrap();
    let mut command = run.stado();
    command.args(["product", "deliver", "schema", "--engine", "sqlite"]);
    run.refuse(command, "no delivery");
    assert_eq!(
        fs::read(run.output.join("schema-verify/scratch.sqlite")).unwrap(),
        before
    );
    run.pass();
}

#[cfg(unix)]
#[test]
fn postgres_delivers_only_verified_bytes_with_atomic_migration_history() {
    let mut run = Run::new();
    let mut database = postgres::Database::start(&mut run);
    database.query(&mut run, "create database schema_verify;");
    database.query(&mut run, "create database schema_delivery;");
    let mut url = url::Url::parse(&database.url).unwrap();
    url.set_path("/schema_verify");
    database.url = url.to_string();
    seed(&run);
    run.bundle();
    fs::write(
        run.source.join("migrations/001_create.sql"),
        "not the built migration;",
    )
    .unwrap();
    let mut command = verify(&run, "postgres");
    command.env("WISENT_SCRATCH_DATABASE_URL", &database.url);
    run.success(command);
    assert_eq!(
        database.query(&mut run, "select value from schema_records where id=7;"),
        "archived"
    );

    url.set_path("/schema_delivery");
    database.url = url.to_string();
    let (archive, digest) = run.release();
    let command = deliver(&run, &archive, &"0".repeat(64), &database.url);
    run.refuse(command, "not the published");
    let mut command = deliver(&run, &archive, &digest, &database.url);
    command.env_remove("WISENT_VERSION");
    run.refuse(command, "WISENT_VERSION");
    let mut command = deliver(&run, &archive, &digest, &database.url);
    command.args(["--migrations", "../outside"]);
    run.refuse(command, "inside the schema bundle");
    assert_eq!(
        database.query(
            &mut run,
            "select count(*) from information_schema.tables where table_schema='public';"
        ),
        "0"
    );

    let command = deliver(&run, &archive, &digest, &database.url);
    let receipt: Value = serde_json::from_str(&run.success(command)).unwrap();
    assert_eq!(receipt["release_sha256"], digest);
    assert_eq!(
        receipt["applied"],
        serde_json::json!(["001_create", "002_insert"])
    );
    assert_eq!(
        database.query(&mut run, "select id || ':' || value from schema_records;"),
        "7:archived"
    );
    let command = deliver(&run, &archive, &digest, &database.url);
    let repeated: Value = serde_json::from_str(&run.success(command)).unwrap();
    assert_eq!(repeated["applied"], serde_json::json!([]));
    assert_eq!(
        repeated["already_applied"],
        serde_json::json!(["001_create", "002_insert"])
    );
    assert_eq!(
        database.query(&mut run, "select count(*) from schema_records;"),
        "1"
    );

    seed(&run);
    fs::write(run.source.join("migrations/003_broken.sql"), BROKEN).unwrap();
    run.bundle();
    let (broken, digest) = run.release();
    let command = deliver(&run, &broken, &digest, &database.url);
    run.refuse(command, "003_broken.sql");
    assert_eq!(database.query(&mut run, "select count(*) from information_schema.tables where table_schema='public' and table_name='must_rollback';"), "0");
    assert_eq!(
        database.query(&mut run, "select count(*) from wisent_schema_migrations;"),
        "2"
    );
    assert_eq!(
        database.query(&mut run, "select value from schema_records where id=7;"),
        "archived"
    );

    url.set_path("/postgres");
    database.url = url.to_string();
    database.query(&mut run, "drop database schema_verify;");
    database.query(&mut run, "drop database schema_delivery;");
    assert_eq!(database.query(&mut run, "select count(*) from pg_database where datname in ('schema_verify', 'schema_delivery');"), "0");
    drop(database);
    let exit: Value =
        serde_json::from_slice(&fs::read(run.root.join("postgres-exit.json")).unwrap()).unwrap();
    assert!(exit["error"].is_null(), "PostgreSQL cleanup failed: {exit}");
    run.pass();
}
