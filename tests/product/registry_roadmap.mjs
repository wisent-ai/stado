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

// Real qualification of a product roadmap's history through `stado product
// registry set --add-roadmap`: the Stado under test writes an isolated
// catalog file of one fictional product, and the persisted record is read
// back after every command. An idea is planned, replaced by a second item and
// recorded as superseded with its decision and replacement, and a third is
// rejected with its decision; every item stays in the roadmap. A decided item
// without a decision, an open item carrying one, a superseded item naming no
// or an unknown replacement, an unknown status and an unknown field are
// refused and leave the file as it was. Required: STADO_BIN, the Stado
// executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-registry-roadmap-"));
const home = join(output, "home");
const catalogPath = join(output, "products.yml");
const report = { started_at: new Date().toISOString(), commands: [], result: "failed" };
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
function run(program, args, environment = process.env) {
	const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: "utf8" });
	report.commands.push({ program, arguments: args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
	return answer;
}
const SUCCESS = run("/usr/bin/true", []).status;
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: home };
const record = [
	"- id: alpha",
	"  name: alpha",
	"  status: preview",
	"  visibility: private",
	"  family: wisent",
	"  owner_repository: example-org/alpha",
	"  description: The alpha test product.",
	"  roadmap: []",
	"  integrations: []",
	"  evidence:",
	"  - example-org/alpha/README.md",
	"  surfaces:",
	"  - kind: cli",
	"    repository: example-org/alpha",
].join("\n");
const REPLACED = "example-org/alpha/DECISIONS.md#replaced";
const REJECTED = "example-org/alpha/DECISIONS.md#rejected";
const set = (...args) =>
	run(stado, ["product", "--catalog", catalogPath, "registry", "set", "alpha", ...args, "--json"], isolated);
function roadmap() {
	const answer = run(stado, ["product", "--catalog", catalogPath, "catalog", "--json"], isolated);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	return JSON.parse(answer.stdout).products.find((product) => product.id === "alpha").roadmap;
}
const fields = (written) => ({ outcome: "a user reaches the result", source: "example-org/alpha/ROADMAP.md", ...written });
const item = (written) => JSON.stringify(fields(written));
function refused(args, pattern, catalogBefore) {
	const answer = set(...args);
	assert.notEqual(answer.status, SUCCESS, `refused write passed: ${args.join(" ")}`);
	assert.match(answer.stderr, pattern);
	assert.equal(readFileSync(catalogPath, "utf8"), catalogBefore, "a refused write changed the catalog");
}
try {
	report.source_revision = run("git", ["rev-parse", "HEAD"]).stdout.trim();
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = { path: stado, sha256: digest(readFileSync(stado)) };
	mkdirSync(home, { recursive: true });
	writeFileSync(catalogPath, `authority:\n  owner: stado\n  rule: test catalog\nproducts:\n${record}\n`);

	const first = { title: "first idea", status: "superseded", decision: REPLACED, superseded_by: "second idea" };
	const second = { title: "second idea", status: "in_progress" };
	const third = { title: "third idea", status: "rejected", decision: REJECTED };
	let answer = set("--add-roadmap", item({ title: "first idea", status: "planned" }));
	assert.equal(answer.status, SUCCESS, answer.stderr);
	answer = set("--add-roadmap", item(second));
	assert.equal(answer.status, SUCCESS, answer.stderr);
	answer = set("--add-roadmap", item(first), "--add-roadmap", item(third));
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(
		roadmap(),
		[fields(first), fields(second), fields(third)],
		"every item, ended or open, stays in the roadmap with its decision",
	);
	const catalog = run(stado, ["product", "--catalog", catalogPath, "catalog", "--json"], isolated);
	assert.equal(catalog.status, SUCCESS, catalog.stderr);
	assert.deepEqual(
		JSON.parse(catalog.stdout).roadmap_statuses,
		[
			{ status: "planned", decided: false },
			{ status: "in_progress", decided: false },
			{ status: "delivered", decided: true },
			{ status: "rejected", decided: true },
			{ status: "superseded", decided: true },
		],
		"the catalog publishes which roadmap statuses end an item",
	);

	const before = readFileSync(catalogPath, "utf8");
	refused(["--add-roadmap", item({ title: "fourth idea", status: "delivered" })], /decision names where that was decided/, before);
	refused(["--add-roadmap", item({ title: "fourth idea", status: "planned", decision: REJECTED })], /carries a decision/, before);
	refused(
		["--add-roadmap", item({ title: "second idea", status: "superseded", decision: REPLACED })],
		/superseded_by names the item of alpha that replaced it/,
		before,
	);
	refused(
		["--add-roadmap", item({ title: "second idea", status: "superseded", decision: REPLACED, superseded_by: "no such idea" })],
		/which is no roadmap item of alpha/,
		before,
	);
	refused(["--add-roadmap", item({ title: "fourth idea", status: "abandoned" })], /invalid roadmap item/, before);
	refused(["--add-roadmap", item({ title: "fourth idea", status: "planned", decided: REJECTED })], /decided is not a field/, before);
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
