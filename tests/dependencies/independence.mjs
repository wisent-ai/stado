import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	constants,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real qualification of `stado product dependencies`: the Stado under test
// reads real git checkouts of an isolated workspace against a catalog file,
// and its answer, refusals and exit status are checked for every standing a
// product the code reaches can have: undeclared, bound, unproven,
// source-missing and independent, plus an absent checkout and an unknown
// product. A comment line and a file under tests/ that name the other product
// are not dependencies. Everything lives below this checkout's ignored .build
// and is removed at the end. Required: STADO_BIN, the Stado executable under
// test.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(
	join(realpathSync(join(root, ".build")), "dependencies-independence-"),
);
const home = join(output, "home");
const workspace = join(home, "workspace");
const catalogPath = join(output, "products.yml");
// The schema version the repository's own catalog declares.
const schema = Number(
	/^schema_version:\s*(?<version>\S+)/m.exec(
		readFileSync(join(root, "catalog/products.yml"), "utf8"),
	).groups.version,
);
const report = {
	started_at: new Date().toISOString(),
	commands: [],
	observations: [],
	result: "failed",
};
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
function run(program, args, environment = process.env) {
	const answer = spawnSync(program, args, {
		cwd: root,
		env: environment,
		encoding: "utf8",
	});
	report.commands.push({
		program,
		arguments: args,
		exit_status: answer.status,
		signal: answer.signal,
		error: answer.error?.message,
		stdout: answer.stdout,
		stderr: answer.stderr,
	});
	return answer;
}
const SUCCESS = run("/usr/bin/true", []).status;
function success(answer) {
	assert.equal(
		answer.status,
		SUCCESS,
		answer.stderr || answer.error?.message || answer.signal,
	);
	return answer.stdout.trim();
}
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace };

function record(id, dependencies) {
	return {
		id,
		name: id,
		status: "preview",
		visibility: "private",
		family: "wisent",
		owner_repository: `example-org/${id}`,
		description: `The ${id} test product.`,
		evidence: [`example-org/${id}/README.md`],
		roadmap: [],
		integrations: [],
		surfaces: [{ kind: "cli", repository: `example-org/${id}` }],
		...(dependencies ? { dependencies } : {}),
	};
}
function catalog(alphaDependencies) {
	// JSON is YAML, so the catalog file is written as JSON.
	writeFileSync(
		catalogPath,
		JSON.stringify({
			schema_version: schema,
			authority: { owner: "stado", rule: "test catalog" },
			products: [
				record("alpha", alphaDependencies),
				record("beta"),
				record("gamma"),
			],
		}),
	);
}
function checkout(name, files) {
	const path = join(workspace, name);
	for (const [file, text] of Object.entries(files)) {
		mkdirSync(dirname(join(path, file)), { recursive: true });
		writeFileSync(join(path, file), text);
	}
	success(run("git", ["init", "--quiet", "--initial-branch=main", path]));
	success(run("git", ["-C", path, "add", "."]));
	success(
		run("git", [
			"-C",
			path,
			"-c",
			"user.name=Dependencies test",
			"-c",
			"user.email=deps@example.invalid",
			"commit",
			"--quiet",
			"-m",
			"Example",
		]),
	);
}
function dependencies(product) {
	const answer = run(
		stado,
		[
			"product",
			"--workspace",
			workspace,
			"--catalog",
			catalogPath,
			"dependencies",
			product,
			"--json",
		],
		isolated,
	);
	const parsed = JSON.parse(answer.stdout);
	report.observations.push({
		product,
		exit_status: answer.status,
		value: parsed,
		stderr: answer.stderr,
	});
	const own = parsed.find((entry) => entry.product === product);
	assert.ok(own, `no report for ${product}: ${answer.stdout}`);
	return { answer, report: own };
}
function reached(found, other) {
	const entry = found.report.dependencies.find(
		(dependency) => dependency.product === other,
	);
	assert.ok(
		entry,
		`no dependency on ${other}: ${JSON.stringify(found.report)}`,
	);
	return entry;
}
try {
	report.source_revision = success(run("git", ["rev-parse", "HEAD"]));
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = {
		path: stado,
		sha256: digest(readFileSync(stado)),
		version: success(run(stado, ["--version"])),
	};
	checkout("alpha", {
		"src/main.rs":
			'fn main() {\n    let _ = std::process::Command::new("beta").status();\n}\n',
		"src/standalone.rs": "pub fn without_beta() {}\n",
		"src/notes.rs":
			'// std::process::Command::new("gamma") is how gamma would be called\n',
		"tests/calls.rs":
			'fn t() { let _ = std::process::Command::new("gamma").status(); }\n',
	});
	checkout("beta", { "src/main.rs": "fn main() {}\n" });

	catalog();
	let found = dependencies("alpha");
	assert.notEqual(
		found.answer.status,
		SUCCESS,
		"an undeclared dependency passed",
	);
	assert.equal(found.report.independent, false);
	assert.equal(reached(found, "beta").standing, "undeclared");
	assert.deepEqual(reached(found, "beta").channels, {
		program: reached(found, "beta").sites.length,
	});
	assert.equal(
		found.report.dependencies.some(
			(dependency) => dependency.product === "gamma",
		),
		false,
		"a comment or a tests/ file was read as a dependency",
	);
	assert.match(
		found.answer.stderr,
		/alpha cannot be shown to run without beta: the catalog declares no dependency of alpha on beta, so nothing says what alpha does without it; first reached at example-org\/alpha\/src\/main\.rs:/,
	);

	catalog([{ product: "beta", surface: "cli" }]);
	found = dependencies("alpha");
	assert.notEqual(found.answer.status, SUCCESS);
	assert.equal(reached(found, "beta").standing, "bound");
	assert.match(
		found.answer.stderr,
		/alpha needs the beta cli with no alternative and no bound_because saying why/,
	);

	catalog([
		{
			product: "beta",
			surface: "cli",
			bound_because: "alpha is a surface of beta and not a separate product",
		},
	]);
	found = dependencies("alpha");
	assert.notEqual(found.answer.status, SUCCESS);
	assert.equal(reached(found, "beta").standing, "bound");
	assert.equal(
		reached(found, "beta").because,
		"alpha is a surface of beta and not a separate product",
	);
	assert.match(
		found.answer.stderr,
		/alpha needs the beta cli by design: alpha is a surface of beta and not a separate product/,
	);
	assert.doesNotMatch(found.answer.stderr, /no bound_because/);

	catalog([
		{
			product: "beta",
			surface: "cli",
			alternative: "alpha runs its own copy",
			bound_because: "alpha is a surface of beta",
		},
	]);
	found = dependencies("alpha");
	assert.notEqual(found.answer.status, SUCCESS);
	assert.match(
		found.answer.stderr,
		/declares bound_because beside an alternative/,
	);

	catalog([
		{ product: "beta", surface: "cli", alternative: "alpha runs its own copy" },
	]);
	found = dependencies("alpha");
	assert.notEqual(found.answer.status, SUCCESS);
	assert.equal(reached(found, "beta").standing, "unproven");
	assert.match(found.answer.stderr, /names no alternative_source/);

	catalog([
		{
			product: "beta",
			surface: "cli",
			alternative: "alpha runs its own copy",
			alternative_source: "alpha/src/missing.rs",
		},
	]);
	found = dependencies("alpha");
	assert.notEqual(found.answer.status, SUCCESS);
	assert.equal(reached(found, "beta").standing, "source-missing");
	assert.match(
		found.answer.stderr,
		/alternative_source alpha\/src\/missing\.rs is not a file in the workspace/,
	);

	catalog([
		{
			product: "beta",
			surface: "cli",
			alternative: "alpha runs its own copy",
			alternative_source: "alpha/src/standalone.rs",
		},
	]);
	found = dependencies("alpha");
	assert.equal(found.answer.status, SUCCESS, found.answer.stderr);
	assert.equal(found.report.independent, true);
	assert.equal(reached(found, "beta").standing, "independent");
	assert.equal(
		reached(found, "beta").alternative_source,
		"alpha/src/standalone.rs",
	);

	found = dependencies("gamma");
	assert.notEqual(
		found.answer.status,
		SUCCESS,
		"a product whose checkout is absent passed",
	);
	assert.ok(found.report.repositories.every((repository) => !repository.read));
	assert.match(
		found.answer.stderr,
		/gamma: checkout .*gamma of example-org\/gamma is absent, so what it calls is unknown/,
	);

	const unknown = run(
		stado,
		[
			"product",
			"--workspace",
			workspace,
			"--catalog",
			catalogPath,
			"dependencies",
			"delta",
		],
		isolated,
	);
	assert.notEqual(unknown.status, SUCCESS);
	assert.match(unknown.stderr, /names no product delta/);
	report.result = "passed";
} catch (error) {
	report.error = error.stack || String(error);
	throw error;
} finally {
	rmSync(home, { recursive: true, force: true });
	report.finished_at = new Date().toISOString();
	report.scope =
		"Real git checkouts of an isolated workspace read by the Stado under test against a catalog file, for every dependency standing";
	writeFileSync(
		join(output, "report.json"),
		`${JSON.stringify(report, null, 2)}\n`,
		{ mode: constants.S_IRUSR | constants.S_IWUSR },
	);
	console.log(`${report.result}: ${join(output, "report.json")}`);
}
