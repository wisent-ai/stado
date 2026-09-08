//! The offline mode's payload: the fragment for one target, and the refusals
//! that keep anything unquotable out of it.

mod fragment;

use self::fragment::OFFLINE_SNIPPET;

/// The offline fragment for one target, ready to paste.
///
/// Both substitutions land inside single quotes in `sh`, so a value containing
/// a quote would end the literal and turn the rest of the fragment into
/// something else entirely; a multi-line key line would smuggle extra
/// directives into `authorized_keys`. Neither can happen with what
/// [`crate::cli::fleet::key::authorized_keys_line`] produces from a minted
/// ed25519 key, which is why they are refusals and not escapes. Pure.
pub fn offline_snippet(target_name: &str, authorized_line: &str) -> Result<String, String> {
    let line = authorized_line.trim();
    if line.is_empty() {
        return Err("the minted key produced no authorized_keys line".to_string());
    }
    if line.contains('\n') || line.contains('\r') {
        return Err("the minted key produced more than one authorized_keys line".to_string());
    }
    if line.contains('\'') || target_name.contains('\'') {
        return Err(
            "the key line or target name contains a quote, which cannot go into the fragment"
                .to_string(),
        );
    }
    if target_name.is_empty()
        || !target_name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '.' | '_' | '-'))
    {
        return Err(format!(
            "target name '{target_name}' is not usable in the fragment"
        ));
    }
    Ok(OFFLINE_SNIPPET
        .replace("@FLEET_KEY@", line)
        .replace("@TARGET@", target_name))
}
