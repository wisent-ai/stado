//! A transaction on the sync client: committed by `commit`, rolled back
//! when dropped without it. Each step is handed to the client's runtime and
//! waited for as any statement is; the rollback a drop performs is never
//! cancelled, so a cancelled transaction still ends cleanly.

use std::future::Future;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement as SeaStatement,
};

use super::waiting::wait;
use super::{Client, Error, Result, Run};

pub struct Tx<'c> {
    client: &'c Client,
    transaction: Option<Arc<DatabaseTransaction>>,
}

impl<'c> Tx<'c> {
    pub(super) fn new(client: &'c Client, transaction: Arc<DatabaseTransaction>) -> Self {
        Self {
            client,
            transaction: Some(transaction),
        }
    }

    fn held(&self) -> Result<Arc<DatabaseTransaction>> {
        self.transaction
            .clone()
            .ok_or_else(|| Error::Conversion("the transaction has already ended".to_owned()))
    }

    /// Ends the transaction: commits it when `commit`, else rolls it back.
    /// Refused when it has already ended or a statement still holds it.
    fn finish(
        &mut self,
        commit: bool,
    ) -> Result<impl Future<Output = std::result::Result<(), sea_orm::DbErr>> + Send + 'static>
    {
        let Some(shared) = self.transaction.take() else {
            return Err(Error::Conversion(
                "the transaction has already ended".to_owned(),
            ));
        };
        let transaction = match Arc::try_unwrap(shared) {
            Ok(transaction) => transaction,
            Err(still_shared) => {
                // The holder keeps it; this end is refused, not lost.
                self.transaction = Some(still_shared);
                return Err(Error::Conversion(
                    "the transaction is still in use by a statement".to_owned(),
                ));
            }
        };
        Ok(async move {
            if commit {
                transaction.commit().await
            } else {
                transaction.rollback().await
            }
        })
    }

    pub fn commit(mut self) -> Result<()> {
        let ending = self.finish(true)?;
        self.client.wait("commit the transaction", ending)??;
        Ok(())
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        // A committed transaction has ended; one a statement still holds is
        // rolled back by its holder's drop. Neither is a rollback to run.
        if let Ok(ending) = self.finish(false) {
            let _ = wait(
                &self.client.runtime,
                None,
                "roll the transaction back",
                &self.client.name,
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
        Ok(self
            .client
            .wait(what, async move { transaction.query_all(statement).await })??)
    }

    fn exec(&self, statement: SeaStatement) -> Result<u64> {
        let transaction = self.held()?;
        let what = statement.sql.clone();
        Ok(self.client.wait(what, async move {
            transaction
                .execute(statement)
                .await
                .map(|done| done.rows_affected())
        })??)
    }
}
