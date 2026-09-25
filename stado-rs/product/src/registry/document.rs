use crate::{catalog, common::atomic_write};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
use yaml_rust::{
    parser::{Event, MarkedEventReceiver, Parser},
    scanner::Marker,
};

struct Node {
    start: usize,
    end: usize,
    scalar: Option<String>,
    children: Vec<Node>,
}

#[derive(Default)]
struct Tree {
    stack: Vec<Node>,
    root: Option<Node>,
}

impl Tree {
    fn append(&mut self, node: Node) {
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(node);
        } else {
            self.root = Some(node);
        }
    }
}

impl MarkedEventReceiver for Tree {
    fn on_event(&mut self, event: Event, mark: Marker) {
        let line = mark.line().saturating_sub(1);
        match event {
            Event::MappingStart(_) | Event::SequenceStart(_) => self.stack.push(Node {
                start: line,
                end: line,
                scalar: None,
                children: Vec::new(),
            }),
            Event::MappingEnd | Event::SequenceEnd => {
                if let Some(mut node) = self.stack.pop() {
                    node.end = line;
                    self.append(node);
                }
            }
            Event::Scalar(value, _, _, _) => self.append(Node {
                start: line,
                end: line,
                scalar: Some(value),
                children: Vec::new(),
            }),
            Event::Alias(_) => self.append(Node {
                start: line,
                end: line,
                scalar: None,
                children: Vec::new(),
            }),
            _ => {}
        }
    }
}

fn spans(text: &str) -> Result<BTreeMap<String, (usize, usize)>> {
    let mut tree = Tree::default();
    Parser::new(text.chars()).load(&mut tree, true)?;
    let root = tree.root.context("catalog has no YAML document")?;
    let products = root
        .children
        .chunks_exact(2)
        .find(|pair| pair[0].scalar.as_deref() == Some("products"))
        .map(|pair| &pair[1])
        .context("catalog has no products sequence")?;
    let mut spans = BTreeMap::new();
    for (index, record) in products.children.iter().enumerate() {
        let id = record
            .children
            .chunks_exact(2)
            .find(|pair| pair[0].scalar.as_deref() == Some("id"))
            .and_then(|pair| pair[1].scalar.as_ref())
            .context("product identity must be a scalar")?;
        let end = products
            .children
            .get(index + 1)
            .map_or(products.end, |next| next.start);
        if end <= record.start {
            bail!("registry edits require a block-style products sequence; nothing was changed");
        }
        spans.insert(id.clone(), (record.start, end));
    }
    Ok(spans)
}

pub fn commit(path: &Path, original: &str, document: &Value, id: &str, remove: bool) -> Result<()> {
    catalog::validate(document)?;
    let positions = spans(original)?;
    let lines: Vec<_> = original.split_inclusive('\n').collect();
    let rendered = if remove {
        String::new()
    } else {
        serde_yaml::to_string(&vec![catalog::product(document, id)?])?
    };
    let indented = |start: usize| {
        let prefix = " ".repeat(lines[start].len() - lines[start].trim_start().len());
        rendered
            .split_inclusive('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<String>()
    };
    let candidate = if document["products"].as_array().is_some_and(Vec::is_empty) {
        serde_yaml::to_string(document)?
    } else if let Some(&(start, end)) = positions.get(id) {
        format!(
            "{}{}{}",
            lines[..start].concat(),
            indented(start),
            lines[end..].concat()
        )
    } else if let Some(&(start, end)) = positions.values().max_by_key(|(_, end)| *end) {
        format!(
            "{}{}{}",
            lines[..end].concat(),
            indented(start),
            lines[end..].concat()
        )
    } else {
        serde_yaml::to_string(document)?
    };
    let decoded: Value = serde_yaml::from_str(&candidate)?;
    if decoded != *document {
        bail!("structural registry edit did not reproduce the validated document; nothing was changed");
    }
    if fs::read_to_string(path)? != original {
        bail!("catalog changed during the operation; nothing was overwritten");
    }
    atomic_write(path, candidate.as_bytes())
}
