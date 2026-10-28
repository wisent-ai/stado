//! Every command path Stado Desktop runs is a command the CLI has. Desktop's
//! capability operations name their CLI path by hand
//! (`desktop/StadoDesktop/Sources/Stado/CapabilityOperations/Definitions`),
//! so each declared `path: [...]` is read from those sources and its
//! `--help` is run through the real `stado` binary of this checkout in an
//! isolated home: a path the parser does not know fails the journey, naming
//! the operation's file and path and what the parser said.
// The harness is shared with the object-verbs journey; this one checks
// paths, not help listings or parser refusals.
#[allow(dead_code)]
mod harness;

use std::path::{Path, PathBuf};

use harness::Run;

/// Every Swift source under `directory`, depth first.
fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "swift")
        {
            found.push(path);
        }
    }
}

/// Each `path: ["a", "b"]` an operation declares in `text`, as its words.
fn declared_paths(text: &str) -> Vec<Vec<String>> {
    let declaration = regex::Regex::new(r#"path:\s*\[(?P<words>[^\]]*)\]"#).unwrap();
    let word = regex::Regex::new(r#""(?P<word>[^"]+)""#).unwrap();
    declaration
        .captures_iter(text)
        .map(|captured| {
            word.captures_iter(&captured["words"])
                .map(|quoted| quoted["word"].to_string())
                .collect::<Vec<_>>()
        })
        .filter(|words| !words.is_empty())
        .collect()
}

#[test]
fn every_desktop_operation_path_is_a_cli_command() {
    let definitions = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../desktop/StadoDesktop/Sources/Stado/CapabilityOperations/Definitions");
    let mut files = Vec::new();
    sources(&definitions, &mut files);
    assert!(
        !files.is_empty(),
        "no Swift operation definitions under {}",
        definitions.display()
    );

    let mut run = Run::start();
    let mut unknown = Vec::new();
    let mut checked = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for path in declared_paths(&text) {
            let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
            args.push("--help");
            let output = run.stado(&args);
            if !output.status.success() {
                unknown.push(format!(
                    "{}: stado {} — {}",
                    file.display(),
                    path.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            checked.push(path.join(" "));
        }
    }
    assert!(
        !checked.is_empty(),
        "the operation definitions declare no path"
    );
    assert!(
        unknown.is_empty(),
        "Desktop runs paths the CLI does not have:\n{}",
        unknown.join("\n")
    );
    run.passed();
}
