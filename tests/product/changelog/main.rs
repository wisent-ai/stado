//! `stado product changelog` keeps `CHANGELOG.md` editable while a release is
//! prepared and still releases every entry once: a checkout's Unreleased
//! section is spilled into part files no longer than the stated limit, a new
//! entry is written into the emptied section, and the version bump gathers
//! the parts with it into one release section, each heading once, and removes
//! the parts. The refusals a caller can meet name what is wrong. Every step
//! runs the command Stado parses against a real checkout on disk.

use anyhow::{ensure, Context, Result};
use clap::Command;
use serde_json::{json, Value};
use std::{fs, path::Path};

const RELEASED_RANGE: &str = "changelog/first.md";
const UNRELEASED_ENTRIES: &str = "### Added\n\n- **Added first:** the first addition.\n- **Added second:** the second addition,\n  continued on an indented line.\n\n### Fixed\n\n- **Fixed first:** the first repair.\n- **Fixed second:** the second repair.\n- **Fixed third:** the third repair.\n- **Fixed fourth:** the fourth repair.\n- **Fixed fifth:** the fifth repair.\n";
const LATER_ENTRY: &str =
    "### Fixed\n\n- **Fixed after the spill:** written once the section was emptied.\n";

fn changelog(root: &Path, words: &[&str]) -> Result<i32> {
    let root = root.display().to_string();
    let mut arguments = vec!["changelog", "--root", root.as_str()];
    arguments.extend_from_slice(words);
    let command = stado_product::cli::augment(Command::new("product").no_binary_name(true));
    let matches = command
        .try_get_matches_from(arguments.iter().copied())
        .map_err(|error| anyhow::anyhow!("{}", error.render()))?;
    let (_, changelog) = matches
        .subcommand()
        .context("stado product parsed no subcommand")?;
    stado_product::changelog::command::run(changelog)
}

fn refusal(root: &Path, words: &[&str], names: &str) -> Result<String> {
    let cause = match changelog(root, words) {
        Ok(status) => anyhow::bail!(
            "stado product changelog {} was accepted with status {status}",
            words.join(" ")
        ),
        Err(error) => format!("{error:#}"),
    };
    ensure!(
        cause.contains(names),
        "the refusal of {} does not name {names}: {cause}",
        words.join(" ")
    );
    Ok(cause)
}

/// Whether `items` yields exactly one item.
fn once<T>(mut items: impl Iterator<Item = T>) -> bool {
    items.next().is_some() && items.next().is_none()
}

fn unreleased(root: &Path) -> Result<String> {
    let text = fs::read_to_string(root.join("CHANGELOG.md"))?;
    stado_product::changelog::unreleased_entries(&text)
        .context("CHANGELOG.md no longer reads as a changelog with range files")
}

fn linked(line: &str) -> Result<&str> {
    line.split_once("](")
        .and_then(|(_, target)| target.strip_suffix(')'))
        .with_context(|| format!("`{line}` under Unreleased is not a spilled-part link"))
}

fn spill(root: &Path, report: &mut Value) -> Result<String> {
    let changelog_text = format!(
        "# Changelog\n\n## Released\n\n- [first]({RELEASED_RANGE})\n\n## Unreleased\n\n{UNRELEASED_ENTRIES}"
    );
    fs::write(root.join("CHANGELOG.md"), &changelog_text)?;
    let section_lines = UNRELEASED_ENTRIES.lines().count();
    let limit = section_lines.to_string();
    let none = String::new().len().to_string();
    report["refusals"] = json!([
        refusal(root, &["--spill"], "--line-limit")?,
        refusal(root, &["--line-limit", &limit], "--spill")?,
        refusal(
            root,
            &["--spill", "--line-limit", &none],
            "leaves no line for entries"
        )?,
    ]);
    ensure!(
        fs::read_to_string(root.join("CHANGELOG.md"))? == changelog_text,
        "a refused spill changed CHANGELOG.md"
    );
    ensure!(changelog(root, &["--spill", "--line-limit", &limit])? == libc::EXIT_SUCCESS);
    let links = unreleased(root)?;
    report["links"] = json!(links);
    let parts: Vec<String> = links
        .lines()
        .map(|line| {
            let name = linked(line)?;
            fs::read_to_string(root.join(name)).with_context(|| format!("reading spilled {name}"))
        })
        .collect::<Result<_>>()?;
    ensure!(
        !once(parts.iter()) && !parts.is_empty(),
        "a section longer than the limit was spilled into {} part(s)",
        parts.len()
    );
    ensure!(
        links
            .lines()
            .all(|line| line.contains("](changelog/unreleased/part-")),
        "spilled parts are not written into changelog/unreleased/: {links}"
    );
    for part in &parts {
        ensure!(
            part.lines().count() <= section_lines,
            "a spilled part of {} lines passes the limit of {section_lines}",
            part.lines().count()
        );
        let (_, entries) = part
            .split_once("## Unreleased\n")
            .context("a spilled part holds no ## Unreleased section")?;
        ensure!(
            entries.trim_start().starts_with("### "),
            "a spilled part does not open under the heading its entries belong to:\n{part}"
        );
    }
    for entry in UNRELEASED_ENTRIES
        .lines()
        .filter(|line| line.starts_with("- "))
    {
        ensure!(
            once(parts.iter().filter(|part| part.contains(entry))),
            "`{entry}` is not in exactly one spilled part"
        );
    }
    ensure!(
        parts.iter().any(|part| part.contains(
            "- **Added second:** the second addition,\n  continued on an indented line."
        )),
        "an entry was split from its indented line"
    );
    ensure!(
        !links.is_empty(),
        "after a spill, build submit would read the Unreleased section as empty"
    );
    Ok(links)
}

fn gather(root: &Path, links: &str, report: &mut Value) -> Result<()> {
    let spilled = fs::read_to_string(root.join("CHANGELOG.md"))?;
    fs::write(
        root.join("CHANGELOG.md"),
        format!("{}\n\n{LATER_ENTRY}", spilled.trim_end()),
    )?;
    let wide = usize::MAX.to_string();
    ensure!(
        changelog(root, &["--version", "second", "--line-limit", &wide])? == libc::EXIT_SUCCESS
    );
    ensure!(
        unreleased(root)?.is_empty(),
        "the bump left entries under Unreleased"
    );
    for line in links.lines() {
        let name = linked(line)?;
        ensure!(
            !root.join(name).exists(),
            "the bump left the gathered part {name}"
        );
    }
    let range = fs::read_to_string(root.join("changelog/first-second.md"))
        .context("the bump did not extend the newest range into changelog/first-second.md")?;
    report["range"] = json!(range);
    let start = range.find("## second").context("no ## second section")?;
    let end = range.find("## first").context("no ## first section")?;
    let section = &range[start..end];
    ensure!(
        once(section.matches("### Fixed")) && once(section.matches("### Added")),
        "the release section repeats or loses a heading:\n{section}"
    );
    let order = [
        "- **Added first:**",
        "- **Added second:**",
        "### Fixed",
        "- **Fixed first:**",
        "- **Fixed second:**",
        "- **Fixed fifth:**",
        "- **Fixed after the spill:**",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|entry| {
            section
                .find(entry)
                .with_context(|| format!("the release lost `{entry}`"))
        })
        .collect::<Result<_>>()?;
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    ensure!(
        sorted == positions,
        "the release reorders its entries:\n{section}"
    );
    ensure!(
        fs::read_to_string(root.join("CHANGELOG.md"))?
            .contains("- [first – second](changelog/first-second.md)"),
        "the Released link was not moved to the extended range"
    );
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## Released\n\n- [first – second](changelog/first-second.md)\n\n## Unreleased\n\n- [Unreleased (part one)](changelog/unreleased-part-one.md)\n",
    )?;
    report["malformed_link"] = json!(refusal(
        root,
        &["--version", "third", "--line-limit", &wide],
        "unreleased-part-one.md"
    )?);
    Ok(())
}

fn journey(root: &Path, report: &mut Value) -> Result<()> {
    fs::create_dir_all(root.join("changelog"))?;
    fs::write(
        root.join(RELEASED_RANGE),
        "# Changelog first\n\n## first\n\n### Added\n\n- **Released before:** an older entry.\n",
    )?;
    let links = spill(root, report)?;
    gather(root, &links, report)
}

fn main() -> Result<()> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let root = repository
        .join(".build/product-changelog")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root)?;
    let mut report = json!({ "result": "failed", "root": root });
    let result = journey(&root, &mut report);
    match &result {
        Ok(()) => report["result"] = json!("passed"),
        Err(error) => report["error"] = json!(format!("{error:#}")),
    }
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!("report: {}", root.join("report.json").display());
    result
}
