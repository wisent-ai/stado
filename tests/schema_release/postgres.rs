use super::fixture::Run;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

pub struct Database {
    child: Child,
    reader: Option<JoinHandle<()>>,
    root: PathBuf,
    pub url: String,
}

impl Database {
    pub fn start(run: &mut Run) -> Self {
        if let Some(bin) = std::env::var_os("STADO_TEST_POSTGRES_BIN") {
            let mut paths = vec![PathBuf::from(bin)];
            paths.extend(std::env::split_paths(&run.path));
            run.path = std::env::join_paths(paths).unwrap();
        }
        let data = run.root.join("postgres");
        let mut init = run.command("initdb");
        init.args([
            "--no-locale",
            "--auth=trust",
            "--username=schema_test",
            "-D",
        ])
        .arg(&data);
        run.success(init);
        let mut command = run.command("postgres");
        command
            .arg("-D")
            .arg(&data)
            .args(["-h", "", "-k"])
            .arg(&run.root)
            .args(["-p", "5432"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        fs::write(run.root.join("postgres-command.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "program": "postgres", "args": command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>()
        })).unwrap()).unwrap();
        let mut child = command
            .spawn()
            .expect("real PostgreSQL server must be installed");
        let stderr = child.stderr.take().unwrap();
        let (sender, ready) = mpsc::channel();
        let log = run.root.join("postgres.stderr");
        let reader = std::thread::spawn(move || {
            let mut file = File::create(log).unwrap();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                file.flush().unwrap();
                if line.contains("database system is ready to accept connections") {
                    let _ = sender.send(());
                }
            }
        });
        let mut url = url::Url::parse("postgresql://localhost/postgres").unwrap();
        url.set_username("schema_test").unwrap();
        url.query_pairs_mut()
            .append_pair("host", run.root.to_str().unwrap());
        let database = Self {
            child,
            reader: Some(reader),
            root: run.root.clone(),
            url: url.to_string(),
        };
        ready
            .recv()
            .expect("PostgreSQL exited before readiness; see postgres.stderr");
        database
    }

    pub fn query(&self, run: &mut Run, sql: &str) -> String {
        let mut command = run.command("psql");
        command
            .args([
                "--no-psqlrc",
                "--no-password",
                "--tuples-only",
                "--no-align",
                "--quiet",
                "-v",
                "ON_ERROR_STOP=1",
            ])
            .arg(&self.url)
            .args(["--command", sql]);
        run.success(command).trim().to_owned()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let result = self.child.wait();
        let _ = fs::write(self.root.join("postgres-exit.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "pid": self.child.id(), "status": result.as_ref().map(|status| status.to_string()).ok(),
            "error": result.err().map(|error| error.to_string())
        })).unwrap());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
