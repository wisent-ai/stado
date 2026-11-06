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
import { pinnedGit } from "../native/home.mjs";
import { fileURLToPath } from "node:url";

// Real qualification of a product's submit policy through `stado product
// registry set --submit`: the Stado under test writes an isolated catalog
// file of one fictional product that declares none, and the persisted record
// is read back after every command. The product is declared standard with a
// low-risk limit and a review path, then critical; each write replaces the
// whole declaration. A standard product with no low-risk limit, a critical
// one with it, a zero limit, an unknown criticality and an unknown field are
// refused and leave the file as it was. Required: STADO_BIN, the Stado
// executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-registry-submit-"));
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
const isolated = { ...process.env, ...pinnedGit, HOME: home, WISENT_WORKSPACE: home };
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
const set = (...args) =>
	run(stado, ["product", "--catalog", catalogPath, "registry", "set", "alpha", ...args, "--json"], isolated);
function submit() {
	const answer = run(stado, ["product", "--catalog", catalogPath, "catalog", "--json"], isolated);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	return JSON.parse(answer.stdout).products.find((product) => product.id === "alpha").submit;
}
function refused(policy, pattern, catalogBefore) {
	const answer = set("--submit", JSON.stringify(policy));
	assert.notEqual(answer.status, SUCCESS, `refused write passed: ${JSON.stringify(policy)}`);
	assert.match(answer.stderr, pattern);
	assert.equal(readFileSync(catalogPath, "utf8"), catalogBefore, "a refused write changed the catalog");
}
try {
	report.source_revision = run("git", ["rev-parse", "HEAD"]).stdout.trim();
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = { path: stado, sha256: digest(readFileSync(stado)) };
	mkdirSync(home, { recursive: true });
	writeFileSync(catalogPath, `authority:\n  owner: stado\n  rule: test catalog\nproducts:\n${record}\n`);
	assert.equal(submit(), undefined, "the fictional product starts with no submit policy");

	const standard = { criticality: "standard", max_changed_lines: 400, low_risk_max_changed_lines: 40, review_paths: ["hooks/"] };
	let answer = set("--submit", JSON.stringify(standard));
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(submit(), standard, "the standard declaration is persisted as written");

	const critical = { criticality: "critical", max_changed_lines: 2000 };
	answer = set("--submit", JSON.stringify(critical));
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(submit(), critical, "a second write replaces the whole declaration");

	const before = readFileSync(catalogPath, "utf8");
	refused({ criticality: "standard" }, /no low_risk_max_changed_lines/, before);
	refused({ criticality: "critical", low_risk_max_changed_lines: 40 }, /critical beside low_risk_max_changed_lines/, before);
	refused({ criticality: "critical", max_changed_lines: 0 }, /max_changed_lines is 0/, before);
	refused(
		{ criticality: "vital" },
		/alpha: submit is not a submit declaration[\s\S]*unknown variant `vital`, expected `critical` or `standard`/,
		before,
	);
	refused({ criticality: "critical", reviewers: 2 }, /reviewers is not a field/, before);
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
