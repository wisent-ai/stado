//! The image check's launcher rule, over identities read from real files.
//!
//! A unit whose declared program is a shebang script runs the script's
//! interpreter (and often execs the program it starts), so the running image
//! is never the script's own file. `classify_image` must not report that as
//! stale, must still report a launcher that runs an unlinked image, and must
//! keep reporting a compiled program whose declared file was replaced. The
//! identities come from `installed_image` over files written under the
//! crate's build directory, not from numbers chosen here.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use stado::deploy::service::{classify_image, installed_image, ImageIdentity, ImageState};

fn scratch(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("service-images-launcher")
        .join(format!("{name}-{}", std::process::id()));
    fs::create_dir_all(&root).expect("scratch directory");
    root
}

fn identity(path: &std::path::Path) -> ImageIdentity {
    installed_image(path)
        .expect("identity of a file that exists")
        .0
}

#[test]
fn a_launcher_running_its_interpreter_or_exec_target_is_not_stale() {
    let root = scratch("launcher");
    let script = root.join("start-with-skarbiec");
    fs::write(
        &script,
        "#!/bin/sh\nexec \"$(dirname \"$0\")/brama\" \"$@\"\n",
    )
    .unwrap();
    let interpreter = identity(std::path::Path::new("/bin/sh"));
    let exec_target = root.join("brama");
    fs::write(&exec_target, b"compiled program bytes").unwrap();

    let declared = identity(&script);
    assert_eq!(
        classify_image(&interpreter, &declared, None, true),
        None,
        "the interpreter the kernel runs for a shebang script is not a stale image"
    );
    assert_eq!(
        classify_image(&identity(&exec_target), &declared, None, true),
        None,
        "the program the launcher execs is not a stale image either"
    );
    assert_eq!(classify_image(&declared, &declared, None, true), None);
}

#[test]
fn a_compiled_program_whose_file_was_replaced_is_still_stale() {
    let root = scratch("replaced");
    let program = root.join("stado");
    fs::write(&program, b"first build").unwrap();
    let running = identity(&program);
    // A new file at the same path: a different inode, as an install writes.
    let replacement = root.join("stado.new");
    fs::write(&replacement, b"second build").unwrap();
    fs::rename(&replacement, &program).unwrap();
    let installed = identity(&program);
    assert!(!running.is_same_file(&installed));
    match classify_image(&running, &installed, None, false) {
        Some(ImageState::Replaced { .. }) => {}
        other => panic!("a replaced compiled program is stale-unit-image, not {other:?}"),
    }
}

#[test]
fn a_launcher_running_an_unlinked_image_is_still_a_finding() {
    let root = scratch("unlinked");
    let script = root.join("start-web");
    fs::write(&script, "#!/bin/sh\nexec node server.js\n").unwrap();
    let declared = identity(&script);
    // The running image: a file held open and then unlinked, as a process
    // keeps executing bytes no directory entry names any more.
    let gone = root.join("node-old");
    let mut held = fs::File::create(&gone).unwrap();
    held.write_all(b"an interpreter build that was removed")
        .unwrap();
    let running = identity(&gone);
    fs::remove_file(&gone).unwrap();
    use std::os::unix::fs::MetadataExt;
    let unlinked = ImageIdentity {
        links: held.metadata().unwrap().nlink(),
        ..running
    };
    match classify_image(&unlinked, &declared, None, true) {
        Some(ImageState::Unlinked { .. }) => {}
        other => panic!("an unlinked image under a launcher is a finding, not {other:?}"),
    }
}
