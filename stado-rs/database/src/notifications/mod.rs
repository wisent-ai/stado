//! PostgreSQL notifications on the fleet client's existing verified connection.
//! Subscribe before reading authoritative rows. Notifications are transient
//! wakeups, not durable delivery receipts; a lost connection ends the subscription.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sqlx::postgres::{PgListener, PgNotification};

use crate::Error;

pub struct Subscription {
    listener: Option<PgListener>,
    channel: String,
}

/// Listen on an existing Stado-managed PostgreSQL connection. No credentials,
/// alternate endpoint or product-owned connection pool are resolved here.
pub async fn subscribe(
    connection: &DatabaseConnection,
    channel: String,
) -> Result<Subscription, Error> {
    let backend = connection.get_database_backend();
    if backend != DbBackend::Postgres {
        return Err(Error::new("subscribe", format!("database notifications require PostgreSQL; the selected connection uses {backend:?}")));
    }
    if channel.is_empty() || channel.contains('\0') {
        return Err(Error::new(
            "subscribe",
            "the notification channel must be nonempty and contain no NUL",
        ));
    }
    // PostgreSQL otherwise truncates identifiers, silently selecting a different channel.
    let declaration = connection
        .query_one(Statement::from_string(
            backend,
            "SELECT current_setting('max_identifier_length')::bigint AS maximum_bytes".to_owned(),
        ))
        .await
        .map_err(|cause| Error::new("read notification channel limit", cause.to_string()))?
        .ok_or_else(|| {
            Error::new(
                "read notification channel limit",
                "PostgreSQL returned no identifier limit",
            )
        })?;
    let maximum: i64 = declaration
        .try_get("", "maximum_bytes")
        .map_err(|cause| Error::new("read notification channel limit", cause.to_string()))?;
    let maximum = usize::try_from(maximum).map_err(|cause| {
        Error::new(
            "read notification channel limit",
            format!("invalid server identifier limit {maximum}: {cause}"),
        )
    })?;
    if channel.len() > maximum {
        return Err(Error::new("subscribe", format!("notification channel {channel:?} has {} bytes, exceeding the server identifier limit {maximum}; it was not truncated", channel.len())));
    }
    let mut listener = PgListener::connect_with(connection.get_postgres_connection_pool())
        .await
        .map_err(|cause| {
            Error::new(
                "connect notification subscription",
                format!("channel {channel:?}: {cause}"),
            )
        })?;
    listener.eager_reconnect(false);
    listener
        .listen(&channel)
        .await
        .map_err(|cause| Error::new("subscribe", format!("channel {channel:?}: {cause}")))?;
    Ok(Subscription {
        listener: Some(listener),
        channel,
    })
}

impl Subscription {
    /// Whether receiving has ended. An ended handle never reconnects implicitly.
    pub fn is_ended(&self) -> bool {
        self.listener.is_none()
    }

    /// Receive one wakeup. A transport failure permanently ends this handle:
    /// callers must explicitly subscribe again before taking a fresh row snapshot.
    /// Dropping this future cancels the receive; dropping the handle releases its connection.
    pub async fn next(&mut self) -> Result<PgNotification, Error> {
        let listener = self.listener.as_mut().ok_or_else(|| Error::new(
            "receive notification", format!("subscription to {:?} has ended; establish a new subscription before reading a fresh snapshot", self.channel),
        ))?;
        match listener.try_recv().await {
            Ok(Some(notification)) => Ok(notification),
            Ok(None) => {
                self.listener = None;
                Err(Error::new("receive notification", format!("connection to channel {:?} was lost; notifications may be missing; no reconnection was attempted", self.channel)))
            }
            Err(cause) => {
                self.listener = None;
                Err(Error::new(
                    "receive notification",
                    format!(
                        "channel {:?}: {cause}; the subscription has ended",
                        self.channel
                    ),
                ))
            }
        }
    }
}
