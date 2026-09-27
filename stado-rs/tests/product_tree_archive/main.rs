//! `stado product tree-archive` through the real binary on a scratch tree
//! shaped like an `.xcarchive`: two runs give one digest, `tar` lists the
//! root, the reduced modes and the framework link as a link, extraction
//! reproduces every file, and a missing source is refused without output.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tree-archive-{label}"));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let framework = root.join("App.xcarchive/Products/Kit.framework/Versions/A");
    fs::create_dir_all(&framework).unwrap();
    fs::write(framework.join("Kit"), b"binary").unwrap();
    fs::set_permissions(framework.join("Kit"), fs::Permissions::from_mode(0o775)).unwrap();
    fs::write(root.join("App.xcarchive/Info.plist"), b"<plist/>").unwrap();
    fs::set_permissions(
        root.join("App.xcarchive/Info.plist"),
        fs::Permissions::from_mode(0o664),
    )
    .unwrap();
    symlink(
        "A",
        root.join("App.xcarchive/Products/Kit.framework/Versions/Current"),
    )
    .unwrap();
    root
}

fn archive(source: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "tree-archive", "--source"])
        .arg(source)
        .arg("--output")
        .arg(output)
        .output()
        .expect("run stado product tree-archive")
}

fn run(program: &str, arguments: &[&str]) -> String {
    let output = Command::new(program).args(arguments).output().unwrap();
    assert!(
        output.status.success(),
        "{program} {arguments:?}: {output:?}"
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_tree_packs_to_one_digest_with_its_modes_and_links() {
    let root = scratch("pack");
    let first = root.join("out/first.tar.gz");
    let second = root.join("out/second.tar.gz");
    assert!(archive(&root.join("App.xcarchive"), &first)
        .status
        .success());
    assert!(archive(&root.join("App.xcarchive"), &second)
        .status
        .success());
    assert_eq!(
        fs::read(&first).unwrap(),
        fs::read(&second).unwrap(),
        "one tree, one archive"
    );

    let listing = run("tar", &["-tvzf", &first.to_string_lossy()]);
    let lines: Vec<&str> = listing.lines().collect();
    assert!(
        lines[0].starts_with("drwxr-xr-x") && lines[0].ends_with("App.xcarchive/"),
        "{listing}"
    );
    assert!(
        listing.contains("-rw-r--r--") && listing.contains("App.xcarchive/Info.plist"),
        "{listing}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("-rwxr-xr-x") && line.ends_with("Versions/A/Kit")),
        "{listing}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with('l') && line.ends_with("Versions/Current -> A")),
        "{listing}"
    );
    let names: Vec<&str> = lines
        .iter()
        .map(|line| line.rsplit(' ').next().unwrap())
        .collect();
    assert!(names.iter().all(|name| !name.contains("..")), "{listing}");

    let unpacked = root.join("unpacked");
    fs::create_dir_all(&unpacked).unwrap();
    run(
        "tar",
        &[
            "-xzf",
            &first.to_string_lossy(),
            "-C",
            &unpacked.to_string_lossy(),
        ],
    );
    let kit = unpacked.join("App.xcarchive/Products/Kit.framework/Versions/Current/Kit");
    assert_eq!(fs::read(kit).unwrap(), b"binary");
    assert_eq!(
        fs::read(unpacked.join("App.xcarchive/Info.plist")).unwrap(),
        b"<plist/>"
    );
}

#[test]
fn a_missing_source_is_refused_and_nothing_is_written() {
    let root = scratch("missing");
    let output = root.join("out/none.tar.gz");
    let refused = archive(&root.join("Absent.xcarchive"), &output);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("Absent.xcarchive is missing"),
        "{refused:?}"
    );
    assert!(!output.exists());
}
