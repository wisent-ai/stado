use crate::support::{command, releases::Releases, Run};
use anyhow::{ensure, Context, Result};
use serde_json::json;
use stado_product::common::sha256;
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const APPLE_ISSUER_SHA256: &str =
    "e9473d95d06080920600a0101bf47581906ea21810c67b71ad39616be3c55b4b";
const FLEET_CERTIFICATE_ITEM: &str = "desktop-signing-apple-development";

pub struct Credentials {
    home: OsString,
    item: Option<String>,
    issuers: PathBuf,
}

impl Credentials {
    pub fn prepare(run: &mut Run, releases: &Releases) -> Result<Self> {
        let home = env::var_os("HOME").context("the real signing account HOME is required")?;
        let certificate = env::var_os("WISENT_CODESIGN_CERTIFICATE_PEM");
        let private_key = env::var_os("WISENT_CODESIGN_PRIVATE_KEY_PEM");
        ensure!(
            certificate.is_some() == private_key.is_some(),
            "real signing requires both supplied PEM values, or the managed credential item"
        );
        let item = if certificate.is_none() {
            Some(
                env::var("WISENT_CODESIGN_CREDENTIAL_ITEM")
                    .unwrap_or_else(|_| FLEET_CERTIFICATE_ITEM.into()),
            )
        } else {
            ensure!(
                env::var_os("WISENT_CODESIGN_CREDENTIAL_ITEM").is_none(),
                "select one real signing credential source"
            );
            None
        };
        let issuers = if let Some(path) = env::var_os("WISENT_CODESIGN_ISSUERS_FILE") {
            PathBuf::from(path).canonicalize()?
        } else {
            let configuration: serde_json::Value =
                serde_json::from_slice(&fs::read(&releases.config)?)?;
            let namespace = configuration["storage"]["stado"]["namespace"].as_str()
                .context("the selected Stado configuration must declare its immutable signing-input namespace")?;
            let uri = format!("stado://{namespace}/artifacts/native-signing/apple-issuers-{APPLE_ISSUER_SHA256}.pem");
            let path = run.root.join("apple-issuers.pem");
            let mut fetch = Command::new(&run.binary);
            fetch
                .args(["storage", "get", &uri])
                .arg(&path)
                .env("STADO_CONFIG", &releases.config);
            command(run, fetch)?.passed()?;
            let observed = sha256(&path)?;
            ensure!(
                observed == APPLE_ISSUER_SHA256,
                "immutable signing issuer digest differs: {uri}: {observed}"
            );
            fs::write(
                run.evidence.join("signing-input.json"),
                serde_json::to_vec_pretty(&json!({"uri": uri, "sha256": observed}))?,
            )?;
            path
        };
        Ok(Self {
            home,
            item,
            issuers,
        })
    }

    pub fn configure(&self, command: &mut Command) {
        command
            .env("HOME", &self.home)
            .env("WISENT_CODESIGN_ISSUERS_FILE", &self.issuers);
        if let Some(item) = &self.item {
            command.env("WISENT_CODESIGN_CREDENTIAL_ITEM", item);
        }
    }

    pub fn search_list(&self, run: &mut Run) -> Result<String> {
        let mut inspect = Command::new("/usr/bin/security");
        inspect
            .args(["list-keychains", "-d", "user"])
            .env("HOME", &self.home);
        let observed = command(run, inspect)?;
        observed.passed()?;
        Ok(fs::read_to_string(observed.directory.join("stdout.log"))?)
    }

    pub fn accept_duplicate_issuers(
        &self,
        run: &mut Run,
        path: &Path,
        identifier: &str,
    ) -> Result<()> {
        let chain = fs::read(&self.issuers)?;
        let duplicate = run.root.join("duplicated-issuers.pem");
        let mut bytes = chain.clone();
        bytes.push(b'\n');
        bytes.extend_from_slice(&chain);
        fs::write(&duplicate, bytes)?;
        let mut sign = run.product(&[
            "signing",
            "sign",
            path.to_str().context("non-UTF8 test executable")?,
            "--identifier",
            identifier,
            "--hardened-runtime",
            "--json",
        ]);
        self.configure(&mut sign);
        sign.env("WISENT_CODESIGN_ISSUERS_FILE", &duplicate);
        command(run, sign)?.passed()?;
        Ok(())
    }

    pub fn refuse_wrong_key(&self, run: &mut Run, path: &Path, identifier: &str) -> Result<()> {
        let wrong_key = run.root.join("mismatched-private-key.pem");
        let mut generate = Command::new("/usr/bin/openssl");
        generate
            .args([
                "genpkey",
                "-algorithm",
                "EC",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-out",
            ])
            .arg(&wrong_key);
        command(run, generate)?.passed()?;
        let certificate = if let Some(item) = &self.item {
            let mut read = Command::new("skarbiec");
            read.args(["get", item, "--field", "certificate"])
                .env("HOME", &self.home);
            let read = command(run, read)?;
            read.passed()?;
            fs::read_to_string(read.directory.join("stdout.log"))?
        } else {
            env::var("WISENT_CODESIGN_CERTIFICATE_PEM")
                .context("real signing certificate is missing")?
        };
        let before = sha256(path)?;
        let mut sign = run.product(&[
            "signing",
            "sign",
            path.to_str().context("non-UTF8 test executable")?,
            "--identifier",
            identifier,
            "--hardened-runtime",
            "--json",
        ]);
        self.configure(&mut sign);
        sign.env_remove("WISENT_CODESIGN_CREDENTIAL_ITEM")
            .env("WISENT_CODESIGN_CERTIFICATE_PEM", certificate)
            .env(
                "WISENT_CODESIGN_PRIVATE_KEY_PEM",
                fs::read_to_string(&wrong_key)?,
            );
        command(run, sign)?.refused()?;
        ensure!(
            sha256(path)? == before,
            "a mismatched signing key replaced the executable"
        );
        Ok(())
    }
}
