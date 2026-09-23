use ::plist::{Dictionary, Value};

use crate::deploy::DeployError;

/// Parse a launchd definition without coercing dates, data or integers to strings.
/// Retaining the native types lets host consolidation preserve properties it does
/// not replace, including nested launchd policy and whitespace-only arguments.
pub fn parse_plist(text: &str) -> Result<Dictionary, DeployError> {
    if text.trim_start().starts_with("bplist") {
        return Err(DeployError(
            "unit file is a binary property list; convert it with `plutil -convert xml1`"
                .to_string(),
        ));
    }
    Value::from_reader_xml(text.as_bytes())
        .map_err(|error| DeployError(format!("invalid XML property list: {error}")))?
        .into_dictionary()
        .ok_or_else(|| DeployError("launchd unit root is not a dictionary".to_string()))
}

/// `Program` selects the executable even when launchd has a separate argv[0].
pub(crate) fn plist_program(document: &Dictionary) -> Result<Option<&str>, DeployError> {
    let program = match document.get("Program") {
        Some(program) => Some(program),
        None => match document.get("ProgramArguments") {
            Some(arguments) => arguments.as_array().ok_or_else(|| {
                DeployError("ProgramArguments is not an array".to_string())
            })?.first(),
            None => None,
        },
    };
    program.map(|program| {
        program.as_string().filter(|program| !program.is_empty())
            .ok_or_else(|| DeployError("unit declares an empty or non-string program".to_string()))
    }).transpose()
}

/// Replace the startup contract while preserving other native launchd properties.
pub(crate) fn rewrite_plist_startup(
    mut document: Dictionary,
    label: &str,
    arguments: &[String],
    environment: &[(String, String)],
) -> Result<String, DeployError> {
    let program = arguments.first().filter(|program| !program.is_empty())
        .ok_or_else(|| DeployError("resident launchd declaration has no executable".to_string()))?;
    document.insert("Label".to_string(), Value::String(label.to_string()));
    document.insert("Program".to_string(), Value::String(program.clone()));
    document.insert("ProgramArguments".to_string(), Value::Array(
        arguments.iter().cloned().map(Value::String).collect(),
    ));
    let mut env = Dictionary::new();
    for (name, value) in environment {
        env.insert(name.clone(), Value::String(value.clone()));
    }
    document.insert("EnvironmentVariables".to_string(), Value::Dictionary(env));
    let mut output = Vec::new();
    Value::Dictionary(document).to_writer_xml(&mut output)
        .map_err(|error| DeployError(format!("serializing resident launchd definition: {error}")))?;
    String::from_utf8(output)
        .map_err(|error| DeployError(format!("serialized launchd definition is not UTF-8: {error}")))
}
