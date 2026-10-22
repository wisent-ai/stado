import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	constants,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real qualification of `stado product catalog --names ORG`: the Stado under
// test reads the organization's repositories from GitHub through gh, as the
// register does, against an isolated catalog of two fictional products that
// claim real repositories. Product alpha's owner repository has a GitHub
// description. Product beta's owner repository and the repository it claims
// as its desktop surface have none. A fourth repository without a description
// is claimed by neither. The written register must give alpha's row its
// GitHub description, beta's owner row beta's catalog description, the
// surface row "desktop surface of `beta`:" before that description (both
// with `catalog` as their source), and the unclaimed row a description from
// neither source. `--check` must accept the register and refuse one that
// differs.
//
// Precondition, refused by name when unmet: ORG has an active repository
// with a GitHub description and three without one. GitHub descriptions are
// the organization's to set, so the journey reads them and does not change
// them.
//
// Required: STADO_BIN, the Stado executable under test; NAMES_ORG, the
// GitHub organization whose repositories are read.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const org = process.env.NAMES_ORG;
assert.ok(org, "NAMES_ORG is required: the GitHub organization whose repositories the register reads");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-name-register-"));
const catalogPath = join(output, "products.yml");
const registerPath = join(output, "NAMES.md");
const schema = /^schema_version:\s*(?<version>\S+)/m.exec(
	readFileSync(join(root, "catalog/products.yml"), "utf8"),
).groups.version;
const report = { started_at: new Date().toISOString(), commands: [], result: "failed" };
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
function run(program, args) {
	const answer = spawnSync(program, args, { cwd: root, encoding: "utf8" });
	report.commands.push({
		program,
		arguments: args,
		exit_status: answer.status,
		signal: answer.signal,
		stdout: answer.stdout,
		stderr: answer.stderr,
	});
	return answer;
}
// A process succeeded when it started, was ended by no signal and exited with
// POSIX's success status, zero.
const succeeded = (answer) => !answer.error && !answer.signal && !answer.status;
function success(answer) {
	assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
	return answer.stdout.trim();
}
function record(id, owner, surfaces) {
	return [
		`- id: ${id}`,
		`  name: ${id}`,
		"  status: preview",
		"  visibility: private",
		"  family: wisent",
		`  owner_repository: ${owner}`,
		`  description: The ${id} test product, as its catalog record describes it.`,
		"  roadmap: []",
		"  integrations: []",
		"  evidence:",
		`  - ${owner}/README.md`,
		"  surfaces:",
		...surfaces.flatMap(([kind, repository]) => [`  - kind: ${kind}`, `    repository: ${repository}`]),
	].join("\n");
}
// One register row, split into its four cells.
function row(register, name) {
	const line = register.split("\n").find((text) => text.startsWith(`| \`${name}\` |`));
	assert.ok(line, `the register has no row for ${name}`);
	const cells = /^\|(?<cells>.*)\|$/.exec(line).groups.cells;
	return cells.split(" | ").map((cell) => cell.trim());
}
try {
	report.source_revision = success(run("git", ["rev-parse", "HEAD"]));
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	const binary = resolve(stado);
	report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ["--version"])) };
	// The binary names the commit it was built from, with -dirty when its tree
	// differed from it (stado-rs/build.rs). Only a binary built from this
	// checkout's HEAD, clean, qualifies this source: build it from the commit
	// (stado product cargo build --revision), not from a shared working tree.
	const named = /\(rev (?<revision>\S+)\)/.exec(report.stado.version);
	assert.ok(named, `refused: STADO_BIN's --version names no source revision: ${report.stado.version}`);
	assert.equal(
		named.groups.revision,
		report.source_revision,
		`refused: STADO_BIN was built from ${named.groups.revision}, not from the clean commit ${report.source_revision} this journey qualifies`,
	);

	// Every page of the organization's repositories, one JSON object a line,
	// read independently of the gh call the product makes.
	const listing = success(
		run("gh", [
			"api", "--paginate", `orgs/${org}/repos`,
			"--jq", ".[] | {name, nameWithOwner: .full_name, description, isArchived: .archived}",
		]),
	);
	const active = listing
		.split("\n")
		.filter((line) => line.trim())
		.map((line) => JSON.parse(line))
		.filter((repository) => !repository.isArchived);
	const [githubSide] = active.filter((repository) => repository.description?.trim());
	const [owned, surface, unclaimed] = active.filter((repository) => !repository.description?.trim());
	assert.ok(
		githubSide && owned && surface && unclaimed,
		`precondition unmet: ${org} needs an active repository with a GitHub description and three without one`,
	);
	report.repositories = { github: githubSide.nameWithOwner, owned: owned.nameWithOwner,
		surface: surface.nameWithOwner, unclaimed: unclaimed.nameWithOwner };
	writeFileSync(
		catalogPath,
		`schema_version: ${schema}\nauthority:\n  owner: stado\n  rule: test catalog\nproducts:\n${[
			record("alpha", githubSide.nameWithOwner, [["cli", githubSide.nameWithOwner]]),
			record("beta", owned.nameWithOwner, [["cli", owned.nameWithOwner], ["desktop", surface.nameWithOwner]]),
		].join("\n")}\n`,
	);

	success(run(binary, ["product", "--catalog", catalogPath, "catalog", "--names", org, "--output", registerPath]));
	const register = readFileSync(registerPath, "utf8");
	assert.match(register, /^\| name \| product \| what it is \| from \|$/m);
	assert.deepEqual(row(register, githubSide.name), [
		`\`${githubSide.name}\``,
		"`alpha`",
		githubSide.description.trim().split(/\s+/).join(" ").replaceAll("|", "\\|"),
		"GitHub",
	]);
	assert.deepEqual(row(register, owned.name), [
		`\`${owned.name}\``,
		"`beta`",
		"The beta test product, as its catalog record describes it.",
		"catalog",
	]);
	assert.deepEqual(row(register, surface.name), [
		`\`${surface.name}\``,
		"`beta`",
		"desktop surface of `beta`: The beta test product, as its catalog record describes it.",
		"catalog",
	]);
	assert.deepEqual(row(register, unclaimed.name), [
		`\`${unclaimed.name}\``,
		"",
		"(no description on GitHub or in the catalog)",
		"",
	]);

	success(run(binary, ["product", "--catalog", catalogPath, "catalog", "--names", org, "--check", registerPath]));
	writeFileSync(registerPath, `${register}| \`not-a-repository\` |  | edited by hand |  |\n`);
	const refused = run(binary, ["product", "--catalog", catalogPath, "catalog", "--names", org, "--check", registerPath]);
	assert.ok(!refused.error && !refused.signal && refused.status, "--check passed a register edited by hand");
	assert.match(
		refused.stderr,
		new RegExp(`differs from the register of ${org}; regenerate it with --names --output`),
	);
	report.result = "passed";
} catch (error) {
	report.error = error.stack || String(error);
	throw error;
} finally {
	report.finished_at = new Date().toISOString();
	report.scope = "stado product catalog --names --output and --check against an isolated catalog and the organization's real repositories; not Stado Desktop";
	writeFileSync(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`, {
		mode: constants.S_IRUSR | constants.S_IWUSR,
	});
	console.log(`${report.result}: ${join(output, "report.json")}`);
}
