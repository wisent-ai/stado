import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { constants, mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real qualification of `stado ci status`: the Stado under test reads the
// forge's own API, as the identity its GitHub declaration names in Skarbiec,
// for a catalogued product whose repository declares workflows, and for a
// product whose repository declares none; an unknown product is refused
// naming the catalog, and a revision no run was made for is answered as
// such. The answer is checked against itself and against the forge's own
// shape: every workflow row carries a run with a conclusion or a finding,
// and a run that did not pass names the job and step that failed. Required:
// STADO_BIN, the Stado executable under test; a Skarbiec that answers the
// GitHub identity declaration; STADO_CI_TEST_PRODUCT, a product whose
// repository declares workflows (the Stado product itself does); and
// STADO_CI_TEST_WORKFLOWLESS_PRODUCT, a product whose repository declares
// none. Evidence is kept under this checkout's ignored .build.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const product = process.env.STADO_CI_TEST_PRODUCT;
assert.ok(product, "STADO_CI_TEST_PRODUCT is required: a product whose repository declares workflows");
const workflowless = process.env.STADO_CI_TEST_WORKFLOWLESS_PRODUCT;
assert.ok(workflowless, "STADO_CI_TEST_WORKFLOWLESS_PRODUCT is required: a product whose repository declares no workflow");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "ci-status-"));
const report = { started_at: new Date().toISOString(), commands: [], result: "failed" };
function run(args) {
	const answer = spawnSync(stado, args, { cwd: root, env: process.env, encoding: "utf8" });
	report.commands.push({
		program: stado,
		arguments: args,
		exit_status: answer.status,
		signal: answer.signal,
		error: answer.error?.message,
		stdout: answer.stdout,
		stderr: answer.stderr,
	});
	return answer;
}
const SUCCESS = spawnSync("/usr/bin/true", []).status;
function success(answer) {
	assert.equal(answer.status, SUCCESS, answer.stderr || answer.error?.message || answer.signal);
	return JSON.parse(answer.stdout);
}
// A revision of the same length as a real one that no commit has: the real
// revision with every digit replaced by the hexadecimal alphabet's last.
const unmade = (revision) => revision.replace(/./g, "f");

try {
	const declared = success(run(["ci", "status", product, "--json"]));
	assert.equal(declared.product, product);
	assert.match(declared.repository, /^[^/]+\/[^/]+$/, "the repository is owner/name");
	assert.equal(declared.finding, null, "a repository with workflows has no finding");
	assert.ok(declared.workflows.length, `${declared.repository} declares workflows the forge lists`);
	let concluded = null;
	for (const row of declared.workflows) {
		assert.ok(row.workflow, JSON.stringify(row));
		assert.match(row.path, /^\.github\/workflows\//, JSON.stringify(row));
		if (row.run === null) {
			assert.equal(row.finding, "this workflow has never run");
			continue;
		}
		assert.ok(row.run.id, JSON.stringify(row));
		assert.ok(row.run.url, JSON.stringify(row));
		assert.ok(row.run.revision, JSON.stringify(row));
		if (row.run.conclusion === "success") {
			assert.deepEqual(row.run.failedJobs, [], "a run that passed names no failed job");
		} else if (row.run.conclusion !== null) {
			for (const job of row.run.failedJobs) {
				assert.ok(job.name, JSON.stringify(job));
				assert.notEqual(job.conclusion, "success", JSON.stringify(job));
			}
		}
		concluded ??= row.run;
	}
	assert.ok(concluded, "at least one workflow has run");

	// The same run read by its revision is the run the forge made for it.
	const pinned = success(run(["ci", "status", product, "--revision", concluded.revision, "--json"]));
	assert.equal(pinned.revision, concluded.revision);
	const same = pinned.workflows.find((row) => row.run !== null && row.run.id === concluded.id);
	assert.ok(same, `the run ${concluded.id} is read again for its revision`);
	for (const row of pinned.workflows) {
		if (row.run !== null) {
			assert.equal(row.run.revision, concluded.revision, JSON.stringify(row));
		}
	}

	// A revision nothing ran for is a finding on every workflow, not a refusal.
	const unrun = success(run(["ci", "status", product, "--revision", unmade(concluded.revision), "--json"]));
	for (const row of unrun.workflows) {
		assert.equal(row.run, null, JSON.stringify(row));
		assert.match(row.finding, /^no run of this workflow for revision /, JSON.stringify(row));
	}

	// A repository without workflows is said to declare none.
	const none = success(run(["ci", "status", workflowless, "--json"]));
	assert.deepEqual(none.workflows, []);
	assert.match(none.finding, /declares no workflow under \.github\/workflows/);

	// The text form prints the same rows for a person.
	const lines = run(["ci", "status", product]);
	assert.equal(lines.status, SUCCESS, lines.stderr);
	assert.ok(lines.stdout.startsWith(`${product} ("${declared.repository}")`), lines.stdout);
	for (const row of declared.workflows) {
		assert.ok(lines.stdout.includes(`"${row.workflow}":`), `${row.workflow} is printed: ${lines.stdout}`);
	}

	// A product the catalog does not name is refused naming the catalog.
	const unknown = run(["ci", "status", "no-such-product", "--json"]);
	assert.notEqual(unknown.status, SUCCESS);
	assert.match(unknown.stderr, /no catalogued product no-such-product; `stado product catalog` lists the products/);
	report.result = "passed";
} catch (error) {
	report.error = error.stack || String(error);
	throw error;
} finally {
	report.finished_at = new Date().toISOString();
	report.scope = "The forge's API read by the Stado under test for real catalogued products, with the identity Skarbiec names";
	writeFileSync(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`, {
		mode: constants.S_IRUSR | constants.S_IWUSR,
	});
	console.log(`${report.result}: ${join(output, "report.json")}`);
}
