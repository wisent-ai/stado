//! A BigQuery REST answer's rows, read by their schema's column names, and
//! the typed reading of one column.

use serde_json::{json, Map, Value};

/// The rows of a complete answer, each as its schema's column names to the
/// values BigQuery REST returns (every value a string or null). A complete
/// answer with no `rows` holds none; one whose rows do not match its schema
/// is refused.
pub(super) fn named_rows(body: &Value) -> Result<Vec<Map<String, Value>>, String> {
    let rows = match body.get("rows") {
        None => return Ok(Vec::new()),
        Some(Value::Array(rows)) => rows,
        Some(other) => {
            return Err(format!(
                "BigQuery answered rows that are not a list: {other}"
            ))
        }
    };
    let names: Vec<&str> = body["schema"]["fields"]
        .as_array()
        .ok_or_else(|| {
            format!(
                "BigQuery answered rows without a schema: {}",
                body["schema"]
            )
        })?
        .iter()
        .map(|field| {
            field["name"]
                .as_str()
                .ok_or_else(|| format!("BigQuery schema field without a name: {field}"))
        })
        .collect::<Result<_, _>>()?;
    rows.iter()
        .map(|row| {
            let cells = row["f"]
                .as_array()
                .filter(|cells| cells.len() == names.len())
                .ok_or_else(|| {
                    format!("BigQuery row does not match its schema {names:?}: {row}")
                })?;
            Ok(names
                .iter()
                .zip(cells)
                .map(|(name, cell)| (name.to_string(), cell["v"].clone()))
                .collect())
        })
        .collect()
}

/// A numeric column as a JSON number; SQL NULL stays null. A value that is
/// not a number is refused with its column, not read as null.
pub(super) fn bq_num(row: &Map<String, Value>, column: &str) -> Result<Value, String> {
    match row.get(column) {
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::Number(n)) => Ok(json!(n)),
        Some(Value::String(s)) => s.parse::<f64>().map(|n| json!(n)).map_err(|error| {
            format!("BigQuery column {column} holds {s:?}, not a number: {error}")
        }),
        Some(other) => Err(format!(
            "BigQuery column {column} holds {other}, not a number"
        )),
        None => Err(format!("BigQuery answered no column {column}")),
    }
}

/// A text column; SQL NULL stays null.
pub(super) fn bq_str(row: &Map<String, Value>, column: &str) -> Result<Value, String> {
    match row.get(column) {
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(s)) => Ok(json!(s)),
        Some(Value::Number(n)) => Ok(json!(n.to_string())),
        Some(other) => Err(format!("BigQuery column {column} holds {other}, not text")),
        None => Err(format!("BigQuery answered no column {column}")),
    }
}
