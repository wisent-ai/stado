//! The two Python-compatible JSON serializers the queue's small
//! fixed-shape bodies are built with. Bodies lifted out of `queue/mod.rs`
//! unchanged.

/// Serialize a string as a JSON string literal. Used to build the small
/// fixed-shape JSON bodies (priority markers, tombstones, metadata
/// sidecars) with Python `json.dumps` default separators (", " / ": ").
pub(crate) fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

/// Serialize a JSON value byte-compatibly with Python `json.dumps(value)`:
/// default separators (", " between items, ": " after keys) and
/// `ensure_ascii=True` escaping. Used for the capacity broadcasts and the
/// migration sentinel, which Python readers parse with `json.loads`.
pub(crate) fn python_json_dumps(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    use serde::Serialize;

    /// serde_json `Formatter` reproducing CPython's default `json.dumps`
    /// separators.
    struct PythonSeparators;

    impl serde_json::ser::Formatter for PythonSeparators {
        fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }

        fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            writer.write_all(b": ")
        }

        fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }
    }

    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, PythonSeparators);
    value.serialize(&mut serializer)?;
    Ok(crate::models::ensure_ascii(
        &String::from_utf8(buf).expect("serde_json emits UTF-8"),
    ))
}
