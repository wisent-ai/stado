//! The client is a real process; its editor inherits a pipe whose writer stays alive.
use super::{Native, Server};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};

fn observe_exit(pid: u32) -> OwnedFd {
    let raw = unsafe { libc::kqueue() };
    assert!(
        !raw.is_negative(),
        "create process observer: {}",
        std::io::Error::last_os_error()
    );
    let queue = unsafe { OwnedFd::from_raw_fd(raw) };
    let events = [libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: Default::default(),
        udata: std::ptr::null_mut(),
    }];
    let result = unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            events.as_ptr(),
            events.len() as libc::c_int,
            std::ptr::null_mut(),
            Default::default(),
            std::ptr::null(),
        )
    };
    assert!(
        !result.is_negative(),
        "observe editor exit: {}",
        std::io::Error::last_os_error()
    );
    queue
}

fn collect_exit(queue: &OwnedFd, pid: u32) {
    let mut events = [libc::kevent {
        ident: Default::default(),
        filter: Default::default(),
        flags: Default::default(),
        fflags: Default::default(),
        data: Default::default(),
        udata: std::ptr::null_mut(),
    }];
    let result = unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            std::ptr::null(),
            Default::default(),
            events.as_mut_ptr(),
            events.len() as libc::c_int,
            std::ptr::null(),
        )
    };
    assert!(
        !result.is_negative(),
        "collect editor exit: {}",
        std::io::Error::last_os_error()
    );
    assert!(
        events
            .iter()
            .any(|event| event.ident == pid as libc::uintptr_t
                && event.filter == libc::EVFILT_PROC
                && event.fflags & libc::NOTE_EXIT == libc::NOTE_EXIT),
        "the kernel did not report the editor process ending"
    );
}

const CLIENT: &str = "sourcekit::lifetime::client_process";
const READY: &str = "editor-client-ready ";
const PACKAGE: &str = "STADO_EDITOR_TEST_PACKAGE";
const WORKSPACE: &str = "STADO_EDITOR_TEST_WORKSPACE";

#[test]
#[ignore = "process helper invoked by the editor lifetime journey"]
fn client_process() {
    let package = std::env::var(PACKAGE).expect("real canonical package path");
    let workspace = std::env::var(WORKSPACE).expect("real canonical workspace path");
    let mut editor = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args([
            "product",
            "--workspace",
            &workspace,
            "editor",
            "--package-path",
            &package,
            "serve",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start real editor from isolated client");
    println!("\n{READY}{}", editor.id());
    std::io::stdout().flush().expect("publish child identity");
    let status = editor.wait().expect("collect editor exit");
    assert!(status.success(), "editor exited {status}");
}

fn arguments<'a>(workspace: &'a str, package: &'a str, operation: &'a str) -> Vec<&'a str> {
    vec![
        "product",
        "--workspace",
        workspace,
        "editor",
        "--package-path",
        package,
        operation,
    ]
}

fn status(native: &Native, workspace: &str, package: &str) -> Value {
    let mut args = arguments(workspace, package, "status");
    args.push("--json");
    native.json(&args)
}

#[test]
fn parent_exit_closes_editor_with_live_stdin_and_releases_workspace_ownership() {
    let package = std::env::var(PACKAGE)
        .expect("STADO_EDITOR_TEST_PACKAGE must name a real canonical Swift package");
    let workspace = std::env::var(WORKSPACE)
        .expect("STADO_EDITOR_TEST_WORKSPACE must name its existing canonical checkout workspace");
    let physical = std::path::Path::new(&package)
        .canonicalize()
        .expect("resolve actual package");
    let native = Native::new("editor-parent-lifetime");
    let template = native.command(&[]);
    let stderr_path = native.home.join("editor-client.stderr");
    let mut helper = Command::new(std::env::current_exe().expect("test executable"));
    helper
        .args(["--exact", CLIENT, "--ignored", "--nocapture"])
        .env_clear()
        .env(PACKAGE, &package)
        .env(WORKSPACE, &workspace)
        .current_dir(
            template
                .get_current_dir()
                .expect("fixture working directory"),
        );
    for (key, value) in template.get_envs() {
        if let Some(value) = value {
            helper.env(key, value);
        }
    }
    let mut child = helper
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(File::create(&stderr_path).expect("retain lifecycle stderr"))
        .spawn()
        .expect("start isolated editor client");
    let mut output = BufReader::new(child.stdout.take().expect("client stdout"));
    let editor_pid = loop {
        let mut line = String::new();
        output.read_line(&mut line).expect("read client readiness");
        assert!(!line.is_empty(), "client ended before starting its editor");
        if let Some(pid) = line.trim().strip_prefix(READY) {
            break pid.parse::<u32>().expect("observed editor pid");
        }
    };
    let mut server = Server { child, output };
    native.observe("editor protocol readiness", server.request());
    let listed = status(&native, &workspace, &package);
    let instances = listed["instances"].as_array().expect("instance listing");
    let [instance] = instances.as_slice() else {
        panic!("expected this isolated editor: {listed}")
    };
    assert_eq!(instance["pid"], editor_pid);
    assert_eq!(instance["parent_pid"], server.child.id());
    assert_eq!(instance["workspace"], physical.to_string_lossy().as_ref());
    assert!(
        instance["started_at"].as_str().is_some(),
        "missing observed connection start"
    );
    native.observe("owned workspace", listed);

    let args = arguments(&workspace, &package, "serve");
    let exit = observe_exit(editor_pid);
    let duplicate = native.run(&args);
    assert!(
        !duplicate.status.success(),
        "a second editor acquired the same workspace"
    );
    let unchanged = status(&native, &workspace, &package);
    let owner = unchanged["instances"]
        .as_array()
        .expect("instance listing")
        .first()
        .expect("existing owner");
    assert_eq!(owner["pid"], editor_pid);
    native.observe("duplicate refusal preserves owner", unchanged);

    // Only the isolated client is killed. Keeping its stdin writer proves that
    // EOF is not the mechanism that ends the editor in this scenario.
    server
        .child
        .kill()
        .expect("terminate isolated editor client");
    let parent_exit = server.child.wait().expect("reap isolated client");
    assert!(
        server.child.stdin.is_some(),
        "stdin writer must remain open"
    );
    let mut remaining = String::new();
    server
        .output
        .read_to_string(&mut remaining)
        .expect("observe editor stdout closing after parent death");
    collect_exit(&exit, editor_pid);
    let ended = status(&native, &workspace, &package);
    assert_eq!(
        ended["instances"],
        json!([]),
        "parent exit left a workspace owner"
    );
    native.observe(
        "parent exit",
        json!({"parent_exit": parent_exit.code(), "editor_pid": editor_pid,
        "remaining_stdout": remaining, "status": ended, "stdin_writer_held": true}),
    );
    drop(server.child.stdin.take());

    let replacement = native.run(&args);
    assert!(
        replacement.status.success(),
        "workspace was not reusable after parent exit: {}",
        String::from_utf8_lossy(&replacement.stderr)
    );
    assert_eq!(
        status(&native, &workspace, &package)["instances"],
        json!([])
    );
    native.observe(
        "lifecycle stderr",
        json!(fs::read_to_string(stderr_path).expect("read retained lifecycle stderr")),
    );
}
