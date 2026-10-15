//! What this process's environment declares, told apart from a declaration
//! that cannot be read. `std::env::var(..).ok()` reads a variable set to
//! text that is not Unicode as unset, so a value the operator declared was
//! replaced by the default without a word. Only an unset variable declares
//! nothing here; one that is not Unicode is the refusal naming it.

/// The text `variable` holds, as set (each caller decides what a blank value
/// means); `None` when it is unset, and the refusal naming it when it is set
/// to text that is not Unicode.
pub fn declared(variable: &str) -> Result<Option<String>, String> {
    match std::env::var(variable) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(value)) => Err(format!(
            "{variable} is set to text that is not Unicode: {value:?}"
        )),
    }
}
