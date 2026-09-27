//! `stado product documentation index` through the real binary on a scratch
//! documentation site: a stale index is refused by `--check` without a write,
//! a write produces the search index and the home cards the pages imply,
//! `--check` then passes, and a page in an undeclared category or with a
//! canonical URL other than the manifest's is refused. Every path lives under
//! Cargo's target directory for this test binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const HOME: &str = "<html><body><main>\n<!-- canonical-documentation-cards:start -->\nold\n<!-- canonical-documentation-cards:end -->\n</main></body></html>\n";

fn page(url: &str, category: &str, title: &str) -> String {
    format!(
        "<!doctype html><html><head><link rel=\"canonical\" href=\"{url}\">\
         <meta name=\"description\" content=\"About {title} & more\"></head><body>\
         <article><nav>Skip this navigation</nav><p class=\"eyebrow\">Docs / {category}</p>\
         <h1>{title}</h1><p>Body   of\n {title}.</p><script>ignored()</script></article></body></html>"
    )
}

fn site(label: &str, second_category: &str, second_url: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("docs-index-{label}"));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(root.join("docs/start")).unwrap();
    fs::create_dir_all(root.join("docs/cli")).unwrap();
    fs::write(root.join("docs/index.html"), HOME).unwrap();
    fs::write(
        root.join("docs/start/index.html"),
        page(
            "https://example.wisent.com/docs/start/",
            "Start here",
            "Quick start",
        ),
    )
    .unwrap();
    fs::write(
        root.join("docs/cli/index.html"),
        page(second_url, second_category, "The CLI"),
    )
    .unwrap();
    fs::write(
        root.join("docs-manifest.json"),
        r#"{"groups": [{"name": "Start here"}, {"name": "Operate", "categories": ["CLI"]}, {"name": "Examples"}],
            "topics": [
              {"source": "docs/start/index.html", "url": "https://example.wisent.com/docs/start/"},
              {"source": "docs/cli/index.html", "url": "https://example.wisent.com/docs/cli/"}]}"#,
    )
    .unwrap();
    root
}

fn index(root: &Path, check: bool) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(["product", "documentation", "index", "--root"])
        .arg(root);
    if check {
        command.arg("--check");
    }
    command
        .output()
        .expect("run stado product documentation index")
}

fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"))
}

#[test]
fn a_stale_index_is_refused_then_written_then_accepted() {
    let root = site("journey", "CLI", "https://example.wisent.com/docs/cli/");
    let stale = index(&root, true);
    assert!(!stale.status.success(), "{stale:?}");
    assert_eq!(
        report(&stale)["stale"],
        serde_json::json!(["search-index.json", "docs/index.html"])
    );
    assert!(
        !root.join("search-index.json").exists(),
        "--check wrote the index"
    );
    assert_eq!(
        fs::read_to_string(root.join("docs/index.html")).unwrap(),
        HOME
    );

    let written = index(&root, false);
    assert!(written.status.success(), "{written:?}");
    assert_eq!(report(&written)["pages"], 2);
    let entries: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("search-index.json")).unwrap()).unwrap();
    assert_eq!(entries[0]["title"], "Quick start");
    assert_eq!(entries[0]["url"], "/docs/start/");
    assert_eq!(entries[0]["summary"], "About Quick start & more");
    assert_eq!(
        entries[0]["text"],
        "Docs / Start here Quick start Body of Quick start."
    );
    let home = fs::read_to_string(root.join("docs/index.html")).unwrap();
    assert!(
        home.starts_with(
            "<html><body><main>\n<!-- canonical-documentation-cards:start -->\n<section"
        ),
        "{home}"
    );
    assert!(
        home.ends_with(
            "</section>\n<!-- canonical-documentation-cards:end -->\n</main></body></html>\n"
        ),
        "{home}"
    );
    assert!(
        home.contains("<h2>Operate</h2><span>01 topics</span>"),
        "{home}"
    );
    assert!(
        home.contains("<h2>Examples</h2><span>00 topics</span>"),
        "{home}"
    );
    assert!(
        home.contains("data-doc-summary=\"About The CLI &amp; more\"><span>CLI</span>"),
        "{home}"
    );
    assert!(!home.contains("old"), "{home}");

    let accepted = index(&root, true);
    assert!(accepted.status.success(), "{accepted:?}");
    assert_eq!(report(&accepted)["ok"], true);
}

#[test]
fn an_undeclared_category_or_a_wrong_canonical_url_is_refused() {
    let root = site(
        "category",
        "Tutorials",
        "https://example.wisent.com/docs/cli/",
    );
    let refused = index(&root, false);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unknown documentation category \"Tutorials\""),
        "{refused:?}"
    );
    assert!(!root.join("search-index.json").exists());

    let root = site("canonical", "CLI", "https://example.wisent.com/docs/other/");
    let refused = index(&root, false);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("does not match \"https://example.wisent.com/docs/cli/\""),
        "{refused:?}"
    );
    assert!(!root.join("search-index.json").exists());
}
