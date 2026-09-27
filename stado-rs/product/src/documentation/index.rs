//! `stado product documentation index [--root DIR] [--check]`: the search
//! index and the home page's topic cards of a static documentation site,
//! generated from its canonical pages. transcript-lake-landing carried this as
//! `scripts/build_search_index.py`, run by its release quality gate and by
//! Vercel's build command.
//!
//! `docs-manifest.json` lists the `topics`, each with its `source` page and
//! its canonical `url`, and the home page's `groups` in the order they are
//! shown, each with its `name` and the page `categories` it holds besides its
//! own name. Every page must declare that canonical URL, a description, and an
//! `<article>` with an `<h1>` title and a `<p class="eyebrow">` category whose
//! last ` / ` segment names its group. The index is `search-index.json`
//! (title, summary, path, and the article's text outside `nav`, `script` and
//! `style`); the cards replace the one region of `docs/index.html` between the
//! two card markers. `--check` writes nothing and refuses when either file
//! differs from what the pages produce.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use scraper::{ElementRef, Html, Node, Selector};
use serde_json::{json, Value};

const MANIFEST: &str = "docs-manifest.json";
const SEARCH_INDEX: &str = "search-index.json";
const HOME: &str = "docs/index.html";
const CARDS_START: &str = "<!-- canonical-documentation-cards:start -->";
const CARDS_END: &str = "<!-- canonical-documentation-cards:end -->";
const CATEGORY_CLASS: &str = "eyebrow";

static CANONICAL: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(r#"link[rel="canonical"]"#).expect("valid canonical selector")
});
static DESCRIPTION: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(r#"meta[name="description"]"#).expect("valid description selector")
});
static ARTICLE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("article").expect("valid article selector"));
/// Page chrome inside an article that is not the article's text.
static NOT_TEXT: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("nav, script, style").expect("valid chrome selector"));
static TITLE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1").expect("valid title selector"));

#[derive(Default)]
struct Article {
    title: Vec<String>,
    text: Vec<String>,
    category: Vec<String>,
}

fn walk(element: ElementRef, in_title: bool, in_category: bool, article: &mut Article) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                article.text.push(text.to_string());
                if in_title {
                    article.title.push(text.to_string());
                }
                if in_category {
                    article.category.push(text.to_string());
                }
            }
            Node::Element(value) => {
                let child = ElementRef::wrap(child).expect("an element node wraps");
                if NOT_TEXT.matches(&child) {
                    continue;
                }
                let title = in_title || TITLE.matches(&child);
                let category = in_category
                    || (value.name() == "p" && value.attr("class") == Some(CATEGORY_CLASS));
                walk(child, title, category, article);
            }
            _ => {}
        }
    }
}

fn collapse(parts: &[String]) -> String {
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

struct Group {
    name: String,
    categories: Vec<String>,
    members: Vec<(String, Value)>,
}

fn groups(manifest: &Value) -> Result<Vec<Group>> {
    let declared = manifest["groups"]
        .as_array()
        .with_context(|| format!("{MANIFEST} declares no groups for the home page"))?;
    declared
        .iter()
        .map(|group| {
            let name = group["name"]
                .as_str()
                .with_context(|| format!("{MANIFEST}: every group has a name"))?;
            let categories = group["categories"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok(Group {
                name: name.to_string(),
                categories,
                members: Vec::new(),
            })
        })
        .collect()
}

fn build(root: &Path) -> Result<(Vec<Value>, Vec<Group>)> {
    let manifest_path = root.join(MANIFEST);
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )
    .with_context(|| format!("{} is not JSON", manifest_path.display()))?;
    let topics = manifest["topics"]
        .as_array()
        .with_context(|| format!("{MANIFEST} has no topics list"))?;
    let mut groups = groups(&manifest)?;
    let root = root.canonicalize()?;
    let mut entries = Vec::new();
    for topic in topics {
        let (Some(source), Some(url)) = (topic["source"].as_str(), topic["url"].as_str()) else {
            bail!("{MANIFEST}: every topic names its source and url");
        };
        let path = root
            .join(source)
            .canonicalize()
            .with_context(|| format!("{source}: page is missing"))?;
        if !path.starts_with(&root) {
            bail!("{source}: source leaves the website checkout");
        }
        let page = Html::parse_document(&fs::read_to_string(&path)?);
        let canonical = page
            .select(&CANONICAL)
            .filter_map(|link| link.value().attr("href"))
            .last();
        if canonical != Some(url) {
            bail!("{source}: canonical URL {canonical:?} does not match {url:?}");
        }
        let summary = page
            .select(&DESCRIPTION)
            .filter_map(|meta| meta.value().attr("content"))
            .last()
            .unwrap_or_default();
        let mut article = Article::default();
        for element in page.select(&ARTICLE) {
            walk(element, false, false, &mut article);
        }
        let title = collapse(&article.title);
        let text = collapse(&article.text);
        if title.is_empty() || summary.is_empty() || text.is_empty() {
            bail!("{source}: missing article, title, or description");
        }
        let category = article
            .category
            .join(" ")
            .rsplit(" / ")
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let Some(group) = groups
            .iter_mut()
            .find(|group| group.name == category || group.categories.contains(&category))
        else {
            bail!("{source}: unknown documentation category {category:?}");
        };
        let path = url::Url::parse(url)
            .with_context(|| format!("{source}: {url} is not a URL"))?
            .path()
            .to_string();
        let entry = json!({"title": title, "summary": summary, "url": path, "text": text});
        group.members.push((category, entry.clone()));
        entries.push(entry);
    }
    Ok((entries, groups))
}

fn cards(root: &Path, groups: &[Group]) -> Result<String> {
    let mut sections = Vec::new();
    for group in groups {
        let mut cards = String::new();
        for (category, entry) in &group.members {
            let field = |key: &str| html_escape(entry[key].as_str().unwrap_or_default());
            let (title, summary, url) = (field("title"), field("summary"), field("url"));
            cards.push_str(&format!(
                "<a class=\"doc-card\" href=\"{url}\" data-doc-title=\"{title}\" \
                 data-doc-summary=\"{summary}\"><span>{}</span>\
                 <h3>{title}</h3><p>{summary}</p><b aria-hidden=\"true\">→</b></a>",
                html_escape(category)
            ));
        }
        sections.push(format!(
            "<section class=\"docs-card-group\"><div class=\"group-title\">\
             <h2>{}</h2><span>{:02} topics</span></div>\
             <div class=\"docs-card-grid\">{cards}</div></section>",
            html_escape(&group.name),
            group.members.len()
        ));
    }
    let path = root.join(HOME);
    let current =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if current.matches(CARDS_START).count() != 1 || current.matches(CARDS_END).count() != 1 {
        bail!("{HOME} must declare exactly one generated card region");
    }
    let (before, remainder) = current.split_once(CARDS_START).expect("one start marker");
    let (_, after) = remainder.split_once(CARDS_END).expect("one end marker");
    Ok(format!(
        "{before}{CARDS_START}\n{}\n{CARDS_END}{after}",
        sections.join("\n")
    ))
}

pub fn report(root: Option<&str>, check: bool) -> Result<Value> {
    let root = PathBuf::from(root.unwrap_or("."));
    let (entries, groups) = build(&root)?;
    let outputs = [
        (
            SEARCH_INDEX,
            format!("{}\n", serde_json::to_string(&entries)?),
        ),
        (HOME, cards(&root, &groups)?),
    ];
    let mut stale = Vec::new();
    for (name, expected) in &outputs {
        let path = root.join(name);
        if check {
            if fs::read_to_string(&path).ok().as_deref() != Some(expected.as_str()) {
                stale.push(*name);
            }
        } else {
            fs::write(&path, expected).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(json!({
        "ok": stale.is_empty(),
        "pages": entries.len(),
        "mode": if check { "check" } else { "write" },
        "stale": stale,
        "remedy": if stale.is_empty() {
            Value::Null
        } else {
            json!("run stado product documentation index to regenerate them")
        },
    }))
}
