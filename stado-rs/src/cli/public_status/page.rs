//! The status page a person reads: every published component with its state,
//! since when, and the last failed check with the answer that failed it; or,
//! when the published document cannot be read, the reason in its own words.

use serde_json::Value;

/// `text` safe inside HTML element content and attribute values.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// A field as it reads on the page: a string as it is, anything else as JSON.
fn field(value: &Value) -> String {
    match value.as_str() {
        Some(text) => escaped(text),
        None => escaped(&value.to_string()),
    }
}

fn frame(body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Wisent status</title>\n<style>\n\
         body {{ font-family: system-ui, sans-serif; margin: 2rem auto; max-width: 60rem; \
         padding: 0 1rem; color: #1a1a1a; }}\n\
         table {{ border-collapse: collapse; width: 100%; }}\n\
         th, td {{ text-align: left; vertical-align: top; padding: .5rem; \
         border-bottom: 1px solid #ddd; }}\n\
         .up {{ color: #116329; font-weight: 600; }}\n\
         .down {{ color: #a40e26; font-weight: 600; }}\n\
         .error {{ font-family: ui-monospace, monospace; font-size: .85rem; \
         word-break: break-word; }}\n\
         </style>\n</head>\n<body>\n<h1>Wisent status</h1>\n{body}</body>\n</html>\n"
    )
}

/// The page for a published document.
pub(super) fn published(document: &Value) -> String {
    let mut rows = String::new();
    for row in document["components"].as_array().into_iter().flatten() {
        let failure = match row["last_failure"].as_object() {
            Some(failure) => format!(
                "{}<div class=\"error\">{}</div>",
                field(&failure["at"]),
                field(&failure["error"])
            ),
            None => "none recorded".to_owned(),
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td>{}</td></tr>\n",
            field(&row["title"]),
            field(&row["state"]),
            field(&row["state"]),
            field(&row["since"]),
            failure
        ));
    }
    frame(&format!(
        "<p>Checked at {} from {}.</p>\n<table>\n<thead><tr><th>Component</th><th>State</th>\
         <th>Since</th><th>Last failed check</th></tr></thead>\n<tbody>\n{rows}</tbody>\n\
         </table>\n",
        field(&document["checked_at"]),
        field(&document["checked_by"])
    ))
}

/// The page for a document that could not be read, saying why.
pub(super) fn unread(cause: &str) -> String {
    frame(&format!(
        "<p>The state of the components cannot be shown: the status Stado publishes could not \
         be read.</p>\n<p class=\"error\">{}</p>\n",
        escaped(cause)
    ))
}
