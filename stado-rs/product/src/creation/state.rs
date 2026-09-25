use crate::common::{lock, Runtime};
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    path::PathBuf,
};

pub struct Journal {
    connection: Connection,
    _writer: File,
}

impl Journal {
    pub fn open(runtime: &Runtime) -> Result<Self> {
        let explicit = std::env::var_os("STADO_PRODUCT_STATE_DIR").map(PathBuf::from);
        let default = explicit.is_none();
        let root = explicit.unwrap_or_else(|| runtime.home.join(".local/state/stado/product"));
        if !root.is_absolute() {
            bail!("STADO_PRODUCT_STATE_DIR must be absolute");
        }
        // Requests created while this code was the `wisent-products` program
        // live under its name; they move once, so `--status` and `--resume`
        // keep answering for them.
        let former = runtime.home.join(".local/state/wisent-products");
        if default && !root.exists() && former.join("creation.sqlite3").is_file() {
            fs::create_dir_all(root.parent().context("creation state has no parent")?)?;
            fs::rename(&former, &root).with_context(|| {
                format!("moving creation state {} to {}", former.display(), root.display())
            })?;
        }
        if !root.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&root)?;
        }
        let metadata = root.symlink_metadata()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("creation state must be an owner-only directory, not a symlink");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                bail!("creation state must be owned by this user and inaccessible to other users");
            }
        }
        let writer = lock(&root.join("creation.lock"))
            .context("creation_busy: another lifecycle operation owns the journal")?;
        let path = root.join("creation.sqlite3");
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("creation database cannot be a symlink");
        }
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS records(key TEXT PRIMARY KEY, data TEXT NOT NULL);")?;
        let journal = Self {
            connection,
            _writer: writer,
        };
        if journal
            .get("schema_version")?
            .is_some_and(|version| version != 1)
        {
            bail!("unsupported product creation journal schema");
        }
        journal.put("schema_version", &json!(1))?;
        Ok(journal)
    }

    pub fn get(&self, key: &str) -> Result<Option<Value>> {
        let text: Option<String> = self
            .connection
            .query_row("SELECT data FROM records WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        text.map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
    }

    pub fn put(&self, key: &str, value: &Value) -> Result<()> {
        self.connection.execute("INSERT INTO records(key,data) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data",
            (key, serde_json::to_string(value)?))?;
        Ok(())
    }
}
