//! Dropping a cached or cancelled connection must close its native SSH session.

use std::ops::{Deref, DerefMut};

use russh::client;

use super::Peer;

pub(crate) struct Session(Option<client::Handle<Peer>>);

impl Session {
    pub(super) fn new(handle: client::Handle<Peer>) -> Self {
        Self(Some(handle))
    }
}

impl Deref for Session {
    type Target = client::Handle<Peer>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("session is present until Drop")
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("session is present until Drop")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let (Some(handle), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move {
                if let Err(error) = handle.disconnect(russh::Disconnect::ByApplication, "host released session", "en").await {
                    eprintln!("stado SSH disconnect failed: {error}");
                }
            });
        }
    }
}
