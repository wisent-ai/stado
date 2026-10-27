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

// Real qualification of sourced rival claims through `stado product registry
// set --add-rival` and `stado product registry check-claims`: the Stado under
// test writes an isolated catalog file of one fictional product, and every
// claim's source is a real https page fetched over the network. A claim whose
// recorded digest is the page's own is current; one recorded with another
// digest is changed; one whose page cannot be fetched is unreachable; the
// check exits unsuccessfully unless every claim is current. A claim without
// an https source, with a time in the future, with a digest that is no
// SHA-256, with a blank verified_by or with an unknown field is refused and
// leaves the file as it was. Required: STADO_BIN, the Stado executable under
// test, and STADO_CLAIM_SOURCE, an https page whose bytes do not change
// during the run.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const source = process.env.STADO_CLAIM_SOURCE;
assert.ok(source?.startsWith("https://"), "STADO_CLAIM_SOURCE is required: an https page stable during the run");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-registry-claims-"));
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
const registry = (...args) => run(stado, ["product", "--catalog", catalogPath, "registry", ...args, "--json"], isolated);
const rival = (claims) =>
	JSON.stringify({ id: "beta", name: "Beta", url: "https://example.com/beta", evidence: "https://example.com/beta/docs", claims });
const claim = (fields) => ({
	capability: "exports a report",
	statement: "Beta exports a report as PDF",
	source,
	read_at: report.started_at,
	...fields,
});
try {
	report.source_revision = run("git", ["rev-parse", "HEAD"]).stdout.trim();
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = { path: stado, sha256: digest(readFileSync(stado)) };
	mkdirSync(home, { recursive: true });
	writeFileSync(catalogPath, `authority:\n  owner: stado\n  rule: test catalog\nproducts:\n${record}\n`);
	const page = Buffer.from(await (await fetch(source)).arrayBuffer());
	const read = digest(page);
	const other = digest(Buffer.from(`${read} is not this page`));
	report.source = { url: source, sha256: read };

	const benchmark = JSON.stringify({ app: "alpha-bench", suites: ["reports"] });
	let answer = registry(
		"set", "alpha", "--benchmark", benchmark,
		"--add-rival", rival([claim({ source_sha256: read, verified_by: "probierz benchmark run example of alpha" })]),
	);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	answer = registry("check-claims", "alpha");
	assert.equal(answer.status, SUCCESS, `a claim whose source still hashes as read failed: ${answer.stdout}${answer.stderr}`);
	let checked = JSON.parse(answer.stdout);
	assert.deepEqual(checked.claims.map((row) => row.state), ["current"]);
	assert.equal(checked.allCurrent, true);

	answer = registry(
		"set", "alpha",
		"--add-rival",
		rival([claim({ source_sha256: other }), claim({ source: "https://unreachable.invalid/beta", source_sha256: read })]),
	);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	answer = registry("check-claims", "alpha");
	assert.notEqual(answer.status, SUCCESS, "a changed and an unreachable source passed the check");
	checked = JSON.parse(answer.stdout);
	assert.deepEqual(checked.claims.map((row) => row.state), ["changed", "unreachable"]);
	assert.match(checked.claims[0].detail, new RegExp(`now hashes to ${read}`));
	assert.equal(checked.allCurrent, false);

	const before = readFileSync(catalogPath, "utf8");
	const refused = (claims, pattern) => {
		const refusal = registry("set", "alpha", "--add-rival", rival(claims));
		assert.notEqual(refusal.status, SUCCESS, `a refused claim was written: ${JSON.stringify(claims)}`);
		assert.match(refusal.stderr, pattern);
		assert.equal(readFileSync(catalogPath, "utf8"), before, "a refused write changed the catalog");
	};
	refused([claim({ source: "http://example.com/beta", source_sha256: read })], /source must be an https page/);
	refused([claim({ read_at: "2999-01-01T00:00:00Z", source_sha256: read })], /is in the future/);
	refused([claim({ source_sha256: "not-a-digest" })], /source_sha256/);
	refused([claim({ source_sha256: read, verified_by: " " })], /verified_by names the Probierz run/);
	refused([claim({ source_sha256: read, extra: "x" })], /expected a list of \{id, name, url, evidence, claims\?\}/);
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
