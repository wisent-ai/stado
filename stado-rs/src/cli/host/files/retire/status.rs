//! How one retirement ended.

/// A dry run found the file ready to move, the file was moved, or there was
/// nothing to retire. Every one of them is a success; a refusal is an error,
/// never a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetireStatus {
    Ready,
    Retired,
    Absent,
}

impl RetireStatus {
    /// One host report word, refused by name when it is not a retire status.
    pub fn parse(word: &str) -> Result<Self, String> {
        <Self as serde::Deserialize>::deserialize(serde_json::Value::String(word.to_string()))
            .map_err(|error| format!("retirement status: {error}"))
    }
}

impl std::fmt::Display for RetireStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}
