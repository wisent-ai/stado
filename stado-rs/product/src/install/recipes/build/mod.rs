mod desktop;
mod evidence;
mod inputs;
pub mod manifest;
mod mounts;
use crate::install::handoff;
use crate::{
    catalog::text,
    common::{atomic_json, checked, platform, relative, step_program, step_search_path, Runtime},
    install::plan::{Placement, Prepared},
    signing, source,
};
use anyhow::{bail, Context, Result};
pub use desktop::desktop;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn step(step: &Value, root: &Path, environment: &BTreeMap<String, String>) -> Result<()> {
    let argv: Vec<&str> = step["argv"]
        .as_array()
        .context("release command requires argv")?
        .iter()
        .map(|value| value.as_str().context("release argv must contain strings"))
        .collect::<Result<_>>()?;
    let (program, arguments) = argv.split_first().context("release argv is empty")?;
    let mut command = Command::new(step_program(program));
    command.args(arguments).envs(environment).current_dir(root);
    // A step that is a script resolves its own programs on PATH: put the
    // directories `step_program` searches ahead of the inherited one, as the
    // release worker does, so `cargo` inside `release/quality.sh` is found
    // under the host agent's minimal PATH too.
    if !environment.contains_key("PATH") {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        if let Some(path) = step_search_path(None, &inherited) {
            command.env("PATH", path);
        }
    }
    checked(&mut command)?;
    Ok(())
}

pub fn release(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
    source_commit: Option<&str>,
) -> Result<Prepared> {
    let id = text(product, "id")?;
    let run = evidence::directory(&runtime.home, root)?;
    let evidence = run.path.clone();
    let area = crate::common::runs::checkout_area(&runtime.home, root)?;
    let recorded = source::snapshot(root, &evidence, &area.join("source-index"))?;
    // The commit the operator pinned, else the checkout's own head. A pinned
    // commit must be one origin/main carries: this installs canonical source,
    // never a commit that exists only here.
    let revision = match source_commit {
        Some(commit) => source::canonical_commit(root, commit)?,
        None => recorded["revision"]
            .as_str()
            .context("source snapshot has no revision")?
            .trim_end_matches("-dirty")
            .to_owned(),
    };
    let key = evidence::failures::key(id, &platform()?, &revision, recipe);
    if let Err(refusal) = evidence::failures::refuse_recorded(&area, &key) {
        fs::remove_dir_all(&evidence)?;
        return Err(refusal);
    }
    // A revision whose build stopped `stado serve` on every start here is
    // held, so product sync does not put back the crash loop a restore of
    // the previous release just ended.
    match crate::serve_stops::held(&runtime.home, &revision) {
        Ok(None) => {}
        Ok(Some(hold)) => {
            fs::remove_dir_all(&evidence)?;
            bail!("{id}: {hold}");
        }
        Err(unreadable) => {
            fs::remove_dir_all(&evidence)?;
            bail!("{id}: whether revision {revision} is held cannot be told: {unreadable}");
        }
    }
    // The signing source is resolved, and a role's credential read, before
    // anything is exported or built: an unreadable source refuses in seconds
    // and names the keychain choice, instead of after the whole build.
    let signing_source = if platform()?.starts_with("darwin-") {
        match signing::Source::resolve(&runtime.signing, None) {
            Ok(source) => Some(source),
            Err(refusal) => {
                fs::remove_dir_all(&evidence)?;
                return Err(refusal.context(format!(
                    "{id}: the signing source for this darwin build cannot be used"
                )));
            }
        }
    } else {
        None
    };
    // An earlier install of this key that failed after its build step kept
    // the build (`evidence::retained`): this install holds that run and
    // resumes at signing instead of exporting, checking and building again.
    let resumed = evidence::retained::find(&area, &key)?;
    let finishing = runtime.finishing.as_deref() == Some(id);
    if finishing && resumed.is_none() {
        fs::remove_dir_all(&evidence)?;
        return Err(handoff::nothing_retained(id, &revision));
    }
    let (_run, evidence) = match &resumed {
        Some(kept) => {
            fs::remove_dir_all(&evidence)?;
            drop(run);
            (crate::common::runs::hold(kept)?, kept.clone())
        }
        None => (run, evidence),
    };
    // Quality, build and signing run in the committed tree, as the release
    // worker's git archive does: an uncommitted edit another session is
    // making in this checkout neither enters the install nor fails it.
    let committed = evidence.join("source");
    let build = evidence::Build::start(&evidence);
    let archive_sha256 = match &resumed {
        Some(kept) => {
            eprintln!(
                "{id}: resumed at signing from the build kept at {}",
                kept.display()
            );
            None
        }
        None => Some(source::export(root, &revision, &committed)?),
    };
    let root = committed.as_path();
    let document = manifest::load(root, text(recipe, "manifest")?)?;
    // A product may publish its releases under another name than its catalog
    // id (Weles releases as `weles-worker`); the installation says which, so
    // the manifest is still checked against exactly one expected name.
    let released_as = recipe
        .get("release_product")
        .and_then(Value::as_str)
        .unwrap_or(id);
    if document["product"] != released_as {
        bail!(
            "release manifest product {} does not match {released_as}, the release product the catalog installation of {id} declares",
            document["product"]
        );
    }
    let platform = platform()?;
    let spec = document["platforms"]
        .get(&platform)
        .with_context(|| format!("{id} has no {platform} release"))?;
    // The release worker hands a build `<source>/.wisent-output` as its
    // output, and every manifest's stage keys are written against that; an
    // output beside the source left transcript-lake's `--target-dir
    // .wisent-output/target` build staging nothing and the install failing
    // with a bare "No such file or directory".
    let output = committed.join(".wisent-output");
    let inputs = evidence.join("inputs");
    if let Some(archive_sha256) = archive_sha256 {
        fs::create_dir_all(&output)
            .with_context(|| format!("creating the build output {}", output.display()))?;
        let mut environment = BTreeMap::from([
            (
                "WISENT_SOURCE_DIR".to_owned(),
                root.to_string_lossy().into_owned(),
            ),
            ("WISENT_SOURCE_COMMIT".to_owned(), revision.clone()),
            ("WISENT_SOURCE_SHA256".to_owned(), archive_sha256),
            (
                "WISENT_OUTPUT_DIR".to_owned(),
                output.to_string_lossy().into_owned(),
            ),
            (
                "WISENT_INPUTS_DIR".to_owned(),
                inputs.to_string_lossy().into_owned(),
            ),
            ("WISENT_PRODUCT".to_owned(), id.to_owned()),
            (
                "WISENT_VERSION".to_owned(),
                manifest::version(root, &document)?,
            ),
            ("WISENT_PLATFORM".to_owned(), platform.clone()),
        ]);
        environment.extend(inputs::materialise(runtime, &document, spec, &inputs)?);
        environment.extend(manifest::secrets(&[&document, spec])?);
        if let Some(quality) = spec.get("quality") {
            let install = (id, revision.as_str(), evidence.as_path());
            evidence::failures::gate(&area, &key, install, quality, |check| {
                step(check, root, &environment)
            })?;
        }
        step(&spec["build"], root, &environment)
            .with_context(|| format!("{id} build failed; evidence: {}", evidence.display()))?;
        // The build passed: a failure from here on costs no rebuild.
        evidence::retained::keep(&area, &key, &evidence, id, &revision)?;
        crate::common::runs::keep(
            &evidence,
            &format!("the build of {id} {revision} passed; the next install of it resumes here"),
        )?;
    } else if !output.is_dir() {
        bail!(
            "{id}: the build kept at {} has no build output at {}; remove the run and install again",
            evidence.display(),
            output.display()
        );
    }
    // An installation of the running product hands the rest to its own build.
    if id == crate::build().product && !finishing {
        let binary = manifest::inside(&output, &handoff::staged_program(spec, id)?)?;
        build.finished();
        return Err(handoff::HandOff { binary, revision }.into());
    }
    if let Some(source) = signing_source {
        signing::stage_with(
            &manifest::inside(root, text(recipe, "manifest")?)?,
            &output,
            &platform,
            &runtime.signing,
            source,
        )?;
    }
    let runtime_binary = document["runtime"]["binary"].as_str().unwrap_or(id);
    let mut placements = Vec::new();
    let mut binaries = 0;
    for (source_name, member) in spec["stage"]
        .as_object()
        .context("release platform has no stage map")?
    {
        let member = member.as_str().context("stage member must be a path")?;
        let member_path = relative(Path::new(member))?;
        let root_binary = member == runtime_binary && member_path.components().count() == 1;
        // A file nested below bin/ (a launcher's sourced stages) is part of the
        // stage a bin/ entry runs from: it is installed at the same relative
        // path so that entry finds it, but it is no command of its own and
        // gets no link on PATH.
        let nested_helper = member.starts_with("bin/") && member_path.components().count() > 2;
        let binary = (member.starts_with("bin/") && !nested_helper) || root_binary;
        // A product's share directory is staged either as its members
        // (`share/<id>/…`) or as the one directory `share/<id>`; the release
        // archive places both, so a source install places both too.
        let shared = format!("share/{id}");
        let share_member = member == shared || member.starts_with(&format!("{shared}/"));
        if !binary && !nested_helper && !share_member {
            continue;
        }
        let source = manifest::inside(&output, source_name).with_context(|| {
            format!(
                "{id} stage key '{source_name}' is relative to output {}; build evidence: {}",
                output.display(),
                evidence.display()
            )
        })?;
        let destination = runtime.home.join(".stado").join(if root_binary {
            format!("bin/{member}")
        } else {
            member.to_owned()
        });
        // A directory directly under bin/ is a resource the commands beside it
        // read (a SwiftPM `<Package>_<Target>.bundle`, which Bundle.module finds
        // next to the executable): it is placed at the same path, whole, and
        // gets no link on PATH.
        let resource_directory = binary && !root_binary && source.is_dir();
        if (binary || nested_helper) && !resource_directory && !source.is_file() {
            bail!("CLI stage member must be a regular file under bin/, or a resource directory beside one: {member}");
        }
        if binary && !resource_directory {
            placements.push(Placement {
                source: destination.clone(),
                destination: runtime
                    .home
                    .join(".local/bin")
                    .join(destination.file_name().unwrap()),
                symbolic: true,
            });
            binaries += 1;
        }
        placements.push(Placement {
            source,
            destination,
            symbolic: false,
        });
    }
    if binaries == 0 {
        bail!("{id} stages no CLI binary into bin/; nothing was installed");
    }
    placements.sort_by_key(|placement| placement.symbolic);
    atomic_json(
        &evidence.join("prepared.json"),
        &json!({"product": id, "source_revision": revision, "placements": placements, "resumed_from": resumed}),
    )?;
    build.finished();
    Ok(Prepared {
        placements,
        source_revision: revision,
        source_directory: Some(root.to_path_buf()),
        release: None,
        scratch: Some(evidence.clone()),
        cache: None,
        resumed_from: resumed,
    })
}
