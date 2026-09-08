//! Vast credential resolution: the two Skarbiec channels the host bridge
//! reads `stado-vast/api_key` through, and the availability probe the CLI
//! gates the auto-list bridge on.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

/// Resolve the Vast API key only from Skarbiec. A missing item means that the
/// provider is unavailable; authorization and transport failures are logged
/// rather than mistaken for an absent credential.
///
/// Two channels, in this order: the configured (control-plane) consumer first,
/// and the host's own agent grant when this host holds no control-plane bearer.
/// The host side of this bridge runs on a worker -- the RTX box carries the Vast
/// daemon -- and `~/.stado/control-plane-skarbiec-token` does not exist there and
/// must not, so asking as `stado-control-plane` could only ever fail with a
/// message about a missing grant file, which says nothing about the real state.
///
/// It said nothing on 2026-08-18 either: this vault holds no `stado-vast` item at
/// all, so the honest error is the 403 the host's own grant now reports. Nothing
/// was rented at the time; ten `vastai/test:bandwidth-test-nvidia` containers in
/// state `Created` are the daemon's own self-tests, and I mistook them plus a
/// local python3 workload for a renter.
pub async fn resolve_vast_api_key() -> String {
    let control_plane_bearer = crate::config::skarbiec_token_file();
    let control_plane_usable =
        !control_plane_bearer.is_empty() && std::path::Path::new(control_plane_bearer).is_file();
    if control_plane_usable {
        match crate::skarbiec::read_string("stado-vast", "api_key").await {
            Ok(value) => return value.unwrap_or_default(),
            Err(err) => {
                eprintln!("[vast] cannot read stado-vast/api_key from Skarbiec: {err}");
                return String::new();
            }
        }
    }
    let url = crate::config::agent_skarbiec_url();
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    if url.is_empty() || consumer.is_empty() || !std::path::Path::new(token_file).is_file() {
        eprintln!(
            "[vast] no usable Skarbiec grant for stado-vast/api_key: no control-plane bearer at \
             {control_plane_bearer} and no agent grant configured on this host"
        );
        return String::new();
    }
    // This host reads its own grant, whose placement is the only thing known
    // about it here: the platform's handoff directory on an agent VM, an
    // operator-provisioned file anywhere else.
    match crate::credential_store::read_string_with(
        url,
        consumer,
        token_file,
        crate::skarbiec::GrantMode::for_grant_file(token_file),
        "stado-vast",
        "api_key",
    )
    .await
    {
        Ok(value) => value.unwrap_or_default(),
        Err(err) => {
            eprintln!(
                "[vast] cannot read stado-vast/api_key as {consumer} (this host's own grant): {err}"
            );
            String::new()
        }
    }
}

/// Python `vast_api_key_available`.
pub async fn vast_api_key_available() -> bool {
    !resolve_vast_api_key().await.is_empty()
}
