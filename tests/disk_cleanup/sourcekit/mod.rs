//! A real editor protocol process stays responsive while the janitor reclaims
//! an isolated volume. No compiler, mocked server or production cleanup runs.
use super::native::Native;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Stdio};

struct Server {
    child: Child,
    output: BufReader<ChildStdout>,
}
impl Server {
    fn request(&mut self) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": "workspace/buildTargets"
        }))
        .expect("encode BSP request");
        let input = self.child.stdin.as_mut().expect("server stdin");
        write!(input, "Content-Length: {}\r\n\r\n", body.len()).expect("write BSP header");
        input.write_all(&body).expect("write BSP request");
        input.flush().expect("flush BSP request");
        loop {
            let mut length = None;
            loop {
                let mut header = String::new();
                self.output.read_line(&mut header).expect("read BSP header");
                assert!(!header.is_empty(), "server exited before its response");
                if header.trim().is_empty() {
                    break;
                }
                if let Some(value) = header.strip_prefix("Content-Length:") {
                    length = Some(value.trim().parse::<u64>().expect("BSP content length"));
                }
            }
            let length = length.expect("BSP Content-Length");
            let mut body = Vec::new();
            self.output
                .by_ref()
                .take(length)
                .read_to_end(&mut body)
                .expect("read BSP response");
            assert_eq!(body.len() as u64, length, "truncated BSP response");
            let response: Value = serde_json::from_slice(&body).expect("BSP JSON");
            if response["id"] == id {
                assert!(
                    response["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("build/initialize")),
                    "{response}"
                );
                return response;
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        if let Err(error) = self.child.kill() {
            eprintln!("isolated SourceKit process could not be ended: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("isolated SourceKit exit could not be collected: {error}");
        }
    }
}

#[test]
fn an_editor_does_not_block_cleanup_or_product_admission() {
    let native = Native::new("sourcekit-concurrency");
    fs::create_dir_all(native.state_dir()).expect("isolated lock directory");
    let stderr_path = native.home.join("sourcekit.stderr");
    // These arguments are CLI protocol identities, not input classifiers.
    let mut child = native
        .command(&[])
        .arg("product")
        .arg("editor")
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(File::create(&stderr_path).expect("retain SourceKit stderr"))
        .spawn()
        .expect("start real SourceKit command");
    let output = BufReader::new(child.stdout.take().expect("server stdout"));
    let mut server = Server { child, output };
    let response = server.request();
    native.observe(
        "sourcekit ready",
        json!({"command": "stado product editor serve",
        "pid": server.child.id(), "response": response}),
    );
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("space-reclaim.lock"))
        .expect("open actual lock");
    fs2::FileExt::try_lock_exclusive(&lock)
        .expect("an answering SourceKit server must not hold the cleanup lock");
    let mut build = native.command(&[]);
    build.arg("product").arg("swift").arg("build");
    let refused = build.output().expect("execute refused build command");
    native.observe(
        "build during cleanup",
        json!({"command": "stado product swift build",
        "status": refused.status.code(), "stdout": String::from_utf8_lossy(&refused.stdout),
        "stderr": String::from_utf8_lossy(&refused.stderr)}),
    );
    assert!(
        !refused.status.success(),
        "compilation must not start during cleanup"
    );
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        refusal.contains("nothing was exported or built"),
        "{refusal}"
    );
    assert!(
        refusal.contains("ownership was not established"),
        "{refusal}"
    );
    let turn = native.state_dir().join("space-reclaim-turn.json");
    fs::write(&turn, b"{").expect("write interrupted turn record in isolated home");
    let damaged = build
        .output()
        .expect("invoke build with damaged turn record");
    let diagnostic = String::from_utf8_lossy(&damaged.stderr);
    native.observe(
        "unreadable cleanup turn",
        json!({
            "command": "stado product swift build", "status": damaged.status.code(),
            "stdout": String::from_utf8_lossy(&damaged.stdout), "stderr": diagnostic
        }),
    );
    assert!(
        !damaged.status.success(),
        "an unreadable turn cannot admit a build"
    );
    assert!(
        diagnostic.contains(&turn.display().to_string()),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("cannot parse"), "{diagnostic}");
    assert_eq!(fs::read(&turn).expect("retain damaged turn evidence"), b"{");
    fs::remove_file(&turn).expect("remove only the test's damaged record");
    fs2::FileExt::unlock(&lock).expect("release isolated cleanup hold");
    let cache = native.home.join("checkout/target");
    native.cache(&cache);
    let user_data = native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        !cache.exists(),
        "the real pass did not reclaim the tagged cache"
    );
    assert!(user_data.is_file(), "cleanup removed user data");
    assert_ne!(report["outcome"], "lock_busy_workloads", "{report}");
    native.observe("sourcekit after cleanup", server.request());
    drop(server.child.stdin.take());
    let status = server.child.wait().expect("collect SourceKit EOF exit");
    native.observe("sourcekit exit", json!({"status": status.code()}));
    assert!(
        status.success(),
        "SourceKit failed to end after its client closed"
    );
    native.observe(
        "sourcekit stderr",
        json!(fs::read_to_string(&stderr_path).expect("read retained server diagnostics")),
    );
}
