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

// Real qualification of `stado product registry set --add-dependency` and
// `--remove-dependency`: the Stado under test writes an isolated catalog file
// of two fictional products, and the persisted file is read back after every
// command: a need is added, replaced by the same product, surface and
// for_surface, kept beside a need of another surface, removed with every need
// of its product, and each refusal leaves the file as it was. Required:
// STADO_BIN, the Stado executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(
	join(realpathSync(join(root, ".build")), "product-registry-dependencies-"),
);
const home = join(output, "home");
const catalogPath = join(output, "products.yml");
const report = {
	started_at: new Date().toISOString(),
	commands: [],
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
		stdout: answer.stdout,
		stderr: answer.stderr,
	});
	return answer;
}
const SUCCESS = run("/usr/bin/true", []).status;
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: home };
function record(id) {
	return [
		`- id: ${id}`,
		`  name: ${id}`,
		"  status: preview",
		"  visibility: private",
		"  family: wisent",
		`  owner_repository: example-org/${id}`,
		`  description: The ${id} test product.`,
		"  roadmap: []",
		"  integrations: []",
		"  evidence:",
		`  - example-org/${id}/README.md`,
		"  surfaces:",
		"  - kind: cli",
		`    repository: example-org/${id}`,
		"  - kind: service",
		`    repository: example-org/${id}`,
	].join("\n");
}
const set = (...args) =>
	run(
		stado,
		["product", "--catalog", catalogPath, "registry", "set", "alpha", ...args, "--json"],
		isolated,
	);
// The persisted record, as Stado itself reads the catalog file back.
function alpha() {
	const answer = run(
		stado,
		["product", "--catalog", catalogPath, "catalog", "--json"],
		isolated,
	);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	return JSON.parse(answer.stdout).products.find(
		(product) => product.id === "alpha",
	);
}
const need = (fields) =>
	JSON.stringify({ product: "beta", surface: "service", ...fields });
try {
	report.source_revision = run("git", ["rev-parse", "HEAD"]).stdout.trim();
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = { path: stado, sha256: digest(readFileSync(stado)) };
	mkdirSync(home, { recursive: true });
	writeFileSync(
		catalogPath,
		`authority:\n  owner: stado\n  rule: test catalog\nproducts:\n${record("alpha")}\n${record("beta")}\n`,
	);

	let answer = set(
		"--add-dependency",
		need({ for_surface: "cli", alternative: "alpha calls its own model endpoint", alternative_source: "alpha/src/endpoint.rs" }),
	);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(alpha().dependencies, [
		{ product: "beta", surface: "service", for_surface: "cli", alternative: "alpha calls its own model endpoint", alternative_source: "alpha/src/endpoint.rs" },
	]);

	answer = set(
		"--add-dependency",
		need({ for_surface: "cli", alternative: "alpha runs a local model", alternative_source: "alpha/src/local.rs" }),
		"--add-dependency",
		need({ for_surface: "service", bound_because: "the alpha service is a surface of beta" }),
	);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(
		alpha().dependencies.map((row) => [row.for_surface, row.alternative ?? row.bound_because]),
		[
			["cli", "alpha runs a local model"],
			["service", "the alpha service is a surface of beta"],
		],
		"a need with the same product, surface and for_surface is replaced, another surface's is kept",
	);

	const before = readFileSync(catalogPath, "utf8");
	answer = set("--add-dependency", need({ for_surface: "cli", alternatives: "typo" }));
	assert.notEqual(answer.status, SUCCESS);
	assert.match(
		answer.stderr,
		/--add-dependency: alternatives is not a field; the fields are product, surface, for_surface, alternative, alternative_source, bound_because/,
	);
	answer = set("--add-dependency", "beta");
	assert.notEqual(answer.status, SUCCESS);
	assert.match(
		answer.stderr,
		/--add-dependency takes one JSON object with product, surface, for_surface, alternative, alternative_source, bound_because/,
	);
	answer = set(
		"--add-dependency",
		need({ for_surface: "cli", alternative: "x", bound_because: "y" }),
	);
	assert.notEqual(answer.status, SUCCESS);
	assert.match(answer.stderr, /declares bound_because beside an alternative/);
	answer = set("--add-dependency", JSON.stringify({ product: "gamma", surface: "cli", alternative: "x" }));
	assert.notEqual(answer.status, SUCCESS, "a need of an unknown product was written");
	assert.equal(readFileSync(catalogPath, "utf8"), before, "a refused write changed the catalog");

	answer = set("--remove-dependency", "beta");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.equal(alpha().dependencies, undefined, "a record without needs keeps an empty list");
	answer = set("--remove-dependency", "beta");
	assert.notEqual(answer.status, SUCCESS);
	assert.match(answer.stderr, /no dependency on beta is declared; nothing was changed/);
	report.result = "passed";
} catch (error) {
	report.error = error.stack || String(error);
	throw error;
} finally {
	rmSync(home, { recursive: true, force: true });
	report.finished_at = new Date().toISOString();
	writeFileSync(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`, {
		mode: constants.S_IRUSR | constants.S_IWUSR,
	});
	console.log(`${report.result}: ${join(output, "report.json")}`);
}
