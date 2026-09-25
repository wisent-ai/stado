use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{BufRead, Write};

pub fn read(input: &mut impl BufRead) -> Result<Option<Value>> {
    let mut length = None;
    let mut headers = false;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            if headers {
                bail!("truncated JSON-RPC headers");
            }
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        headers = true;
        let (name, value) = line.split_once(':').context("invalid JSON-RPC header")?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                bail!("duplicate JSON-RPC Content-Length");
            }
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length.context("JSON-RPC Content-Length is missing")?;
    if length == 0 {
        bail!("JSON-RPC Content-Length must be positive");
    }
    let mut body = Vec::new();
    body.try_reserve_exact(length)
        .context("JSON-RPC body exceeds available memory")?;
    body.resize(length, 0);
    input
        .read_exact(&mut body)
        .context("truncated JSON-RPC body")?;
    let message: Value = serde_json::from_slice(&body)?;
    if !message.is_object() {
        bail!("JSON-RPC message must be an object");
    }
    Ok(Some(message))
}

pub fn write(output: &mut impl Write, mut value: Value) -> Result<()> {
    value["jsonrpc"] = Value::String("2.0".to_owned());
    let body = serde_json::to_vec(&value)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}
