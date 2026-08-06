//! `stado release build` — produce a release archive on a fleet host.
//!
//! `release prepare` signs and publishes an archive; nothing produced one. That
//! missing step is why products reached for a hosted CI service, and why an
//! exhausted quota there stopped releases for all of them.
//!
//! The split between the two verbs is deliberate. Building happens on a host
//! chosen from the registry for the platform being built; signing happens where
//! the release authority's key lives, which is not that host. The archive comes
//! back and `prepare` takes it from there, so the key never travels to a builder.

use clap::Args;
use serde_json::json;

use super::CmdError;

#[derive(Args)]
pub struct ReleaseBuildArgs {
    /// Git remote to build from.
    #[arg(long)]
    pub repo: String,
    /// Tag or revision to check out. Built exactly as published.
    #[arg(long)]
    pub version: String,
    /// Platform identifier, e.g. `darwin-arm64`.
    #[arg(long)]
    pub platform: String,
    /// Build on this registry host instead of one selected for the platform.
    #[arg(long)]
    pub host: Option<String>,
    /// Directory for the archive this leaves behind.
    #[arg(long, default_value = ".")]
    pub output_dir: String,
    #[arg(long)]
    pub json: bool,
}

/// The recipe a product declares, read on the builder after checkout.
///
/// It lives in the product's repository because Stado does not know how to
/// build any particular product and should not learn: the fleet supplies a
/// host, a clean checkout and somewhere to put the result.
const RECIPE_PATH: &str = ".stado/release.json";

const BUILD_BODY: &str = r#"
set -eu
repo=@REPO@
version=@VERSION@
platform=@PLATFORM@
root="$HOME/.stado/build"
work="$root/@SLUG@"

if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi
if [ -z "$stado_bin" ]; then
  echo "STADO_STATUS=failed"
  echo "STADO_DETAIL=this host has no stado to archive the result with"
  exit 1
fi

rm -rf "$work"
mkdir -p "$work"
git clone --quiet --depth 1 --branch "$version" "$repo" "$work/src" 2>/dev/null || {
  echo "STADO_STATUS=failed"
  echo "STADO_DETAIL=cannot check out $version from $repo"
  exit 1
}
cd "$work/src"
revision="$(git rev-parse HEAD)"

recipe="$work/src/@RECIPE@"
if [ ! -f "$recipe" ]; then
  echo "STADO_STATUS=failed"
  echo "STADO_DETAIL=$version declares no @RECIPE@, so this repository does not say how it is built"
  exit 1
fi

# The reader is written out first: a heredoc inside a command substitution
# ends the substitution at the first `)` in the Python, which is a syntax
# error thrown by the shell about code it was never meant to parse.
reader="$work/read-recipe.py"
cat > "$reader" <<'PY'
import json, shlex, sys
recipe = json.load(open(sys.argv[1]))
platform = sys.argv[2]
if platform not in recipe.get("platforms", []):
    print("echo STADO_STATUS=failed")
    print(f"echo STADO_DETAIL={shlex.quote(f'the recipe does not declare platform {platform}')}")
    print("exit 1")
    raise SystemExit(0)
for key in ("product", "build", "binary", "launcher"):
    if not recipe.get(key):
        print("echo STADO_STATUS=failed")
        print(f"echo STADO_DETAIL={shlex.quote(f'the recipe declares no {key}')}")
        print("exit 1")
        raise SystemExit(0)
print(f"product={shlex.quote(recipe['product'])}")
print(f"build_cmd={shlex.quote(recipe['build'])}")
print(f"binary={shlex.quote(recipe['binary'])}")
print(f"launcher={shlex.quote(recipe['launcher'])}")
print(f"minimum_stado={shlex.quote(str(recipe.get('minimum_stado_version','')))}")
sources = recipe.get("sources") or {}
for path, spec in sources.items():
    if "/" in path or not spec.get("repo") or not spec.get("revision"):
        print("echo STADO_STATUS=failed")
        print(f"echo STADO_DETAIL={shlex.quote(f'source {path} needs a repo and an exact revision, and one path segment')}")
        print("exit 1")
        raise SystemExit(0)
source_specs = " ".join(
    f"{shlex.quote(path)}={shlex.quote(spec['repo'])}={shlex.quote(spec['revision'])}"
    for path, spec in sources.items()
)
print(f"source_specs={shlex.quote(source_specs)}")
stage = recipe.get("stage") or {}
pairs = " ".join(f"{shlex.quote(dest)}={shlex.quote(src)}" for dest, src in stage.items())
print(f"stage_pairs={shlex.quote(pairs)}")
PY
eval "$(/usr/bin/python3 "$reader" "$recipe" "$platform")"

# Dependent sources are pinned by exact revision, never by branch: a release
# built twice from the same tag must contain the same dependency, and a moving
# reference makes the archive a function of when it was built.
for spec in ${source_specs:-}; do
  path="${spec%%=*}"
  rest="${spec#*=}"
  src_repo="${rest%%=*}"
  src_rev="${rest#*=}"
  rm -rf "$work/src/$path"
  git clone --quiet "$src_repo" "$work/src/$path" 2>/dev/null || {
    echo "STADO_STATUS=failed"
    echo "STADO_DETAIL=cannot clone dependent source $src_repo"
    exit 1
  }
  git -C "$work/src/$path" checkout --quiet "$src_rev" 2>/dev/null || {
    echo "STADO_STATUS=failed"
    echo "STADO_DETAIL=$src_repo has no revision $src_rev"
    exit 1
  }
  echo "STADO_SOURCE=$path@$src_rev"
done

sh -c "$build_cmd"

stage="$work/stage"
rm -rf "$stage"
for pair in $stage_pairs; do
  dest="${pair%%=*}"
  src="${pair#*=}"
  mkdir -p "$stage/$(dirname "$dest")"
  cp -R "$src" "$stage/$dest"
done

archive="$work/@SLUG@.tar.gz"
rm -f "$archive"
"$stado_bin" storage archive "$stage" "$archive" >/dev/null

echo "STADO_STATUS=built"
echo "STADO_PRODUCT=$product"
echo "STADO_REVISION=$revision"
echo "STADO_BINARY=$binary"
echo "STADO_LAUNCHER=$launcher"
echo "STADO_MINIMUM_STADO=$minimum_stado"
echo "STADO_ARCHIVE=$archive"
"#;

/// Which host builds this platform.
///
/// Asked of the fleet rather than assumed: the machine running the command is
/// rarely the right one, and the host a product is placed on is the wrong one
/// whenever another will do -- building where the thing runs means a bad build
/// takes the service with it.
async fn builder_for(
    platform: &str,
    requested: Option<&str>,
    avoid: Option<&str>,
) -> Result<crate::targets::ComputeTarget, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(name) = requested {
        return registry
            .targets
            .iter()
            .find(|candidate| candidate.name == name)
            .cloned()
            .ok_or_else(|| CmdError::click(format!("{name} is not a host in the registry")));
    }
    let runner = crate::deploy::production_runner();
    let mut fallback: Option<crate::targets::ComputeTarget> = None;
    for candidate in &registry.targets {
        let probe = crate::deploy::host_channel::run_script(
            candidate,
            "set -eu\nprintf 'STADO_PLATFORM=%s-%s\\n' \"$(/usr/bin/uname -s)\" \"$(/usr/bin/uname -m)\"\n",
            &runner,
        )
        .await;
        let Ok(output) = probe else { continue };
        let reported = output
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("STADO_PLATFORM="))
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let matches = match platform {
            "darwin-arm64" => reported == "darwin-arm64",
            "linux-amd64" => reported == "linux-x86_64",
            other => reported == other,
        };
        if !matches {
            continue;
        }
        if Some(candidate.name.as_str()) == avoid {
            fallback = Some(candidate.clone());
            continue;
        }
        return Ok(candidate.clone());
    }
    // Only the placed host can build it. Better than refusing: say so, and let
    // the operator see that this platform has exactly one machine.
    fallback.ok_or_else(|| {
        CmdError::click(format!(
            "no registry host reports platform {platform}, so there is nowhere to build it"
        ))
    })
}

pub async fn build(args: &ReleaseBuildArgs) -> Result<(), CmdError> {
    let slug = format!(
        "{}-{}",
        args.version.replace(['/', ' '], "-"),
        args.platform
    );
    // The host a product already runs on is avoided where another can build.
    let avoid = crate::cli::registry::fetch_document()
        .await
        .ok()
        .and_then(|document| {
            document
                .get("service_directory")?
                .get("services")?
                .as_object()?
                .values()
                .find_map(|entry| entry.get("active_host").and_then(serde_json::Value::as_str))
                .map(str::to_string)
        });
    let target = builder_for(&args.platform, args.host.as_deref(), avoid.as_deref()).await?;
    let runner = crate::deploy::production_runner();
    let script = BUILD_BODY
        .replace("@REPO@", &crate::deploy::shlex_quote(&args.repo))
        .replace("@VERSION@", &crate::deploy::shlex_quote(&args.version))
        .replace("@PLATFORM@", &crate::deploy::shlex_quote(&args.platform))
        .replace("@SLUG@", &slug)
        .replace("@RECIPE@", RECIPE_PATH);
    let output = crate::deploy::host_channel::run_script(&target, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let marker = |tag: &str| -> String {
        output
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix(tag))
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    if !output.ok() || marker("STADO_STATUS=") != "built" {
        return Err(CmdError::click(format!(
            "{}: {}",
            target.name,
            crate::deploy::host_channel::last_error_line(&output, "the build did not finish")
        )));
    }
    let remote_archive = marker("STADO_ARCHIVE=");
    let local = std::path::Path::new(&args.output_dir).join(format!("{slug}.tar.gz"));
    // The archive comes back so that signing happens where the release
    // authority's key is, never on the builder.
    if !crate::deploy::host_channel::target_is_this_host(&target) {
        let ssh_target = target.ssh.clone().unwrap_or_default();
        let mut options = crate::deploy::host_channel::ssh_options(&ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(format!("{ssh_target}:{remote_archive}"));
        argv.push(local.to_string_lossy().to_string());
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "built on {} but could not collect the archive: {}",
                target.name,
                copy.detail()
            )));
        }
    } else {
        std::fs::copy(&remote_archive, &local)?;
    }

    let report = json!({
        "product": marker("STADO_PRODUCT="),
        "version": args.version,
        "platform": args.platform,
        "built_on": target.name,
        "source_revision": marker("STADO_REVISION="),
        "archive": local.to_string_lossy(),
        "binary": marker("STADO_BINARY="),
        "launcher": marker("STADO_LAUNCHER="),
        "minimum_stado_version": marker("STADO_MINIMUM_STADO="),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} {} {} built on {} -> {}",
            report["product"].as_str().unwrap_or_default(),
            args.version,
            args.platform,
            target.name,
            local.display()
        );
        println!(
            "sign and publish it with `stado release prepare`, which takes exactly these fields"
        );
    }
    Ok(())
}
