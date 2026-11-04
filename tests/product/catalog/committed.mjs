import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pinnedGit } from "../../native/home.mjs";
import { fileURLToPath } from "node:url";

// Real qualification of which catalog a reader reads when the catalog's
// checkout is shared with sessions editing it: the committed one. The Stado
// under test runs with an isolated HOME and a workspace below this
// checkout's ignored .build whose `stado` checkout commits a valid catalog
// of one fictional product.
//
// 1. An uncommitted edit that does not validate (the product loses a field
//    the catalog requires): `stado product catalog --json` exits 0, lists
//    the committed record, names the committed revision under
//    `catalog_read`, and says on stderr that the edit does not validate and
//    is not read. `product registry check-claims` reads the same committed
//    record.
// 2. A valid uncommitted edit: still not read; stderr says readers read it
//    once it is committed.
// 3. That edit committed: the next read lists it at the new revision.
// 4. `registry set` on the tracked catalog writes the working tree and its
//    answer says readers see it once committed, naming the commit command.
// 5. A catalog no checkout tracks (`--catalog` outside any checkout) is read
//    as it stands, with `catalog_read` null.
//
// Required: STADO_BIN, the Stado executable under test. Evidence is kept in
// report.json under .build.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-catalog-committed-"));
const home = join(output, "home");
const workspace = join(output, "workspace");
const checkout = join(workspace, "stado");
const catalogFile = join(checkout, "catalog", "products.yml");
const report = { started_at: new Date().toISOString(), commands: [], cases: [], result: "failed" };
function run(program, args, cwd = root) {
	const answer = spawnSync(program, args, {
		cwd,
		env: { ...process.env, ...pinnedGit, HOME: home, WISENT_WORKSPACE: workspace },
		encoding: "utf8",
	});
	report.commands.push({ program, arguments: args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
	return answer;
}
const SUCCESS = run("/usr/bin/true", []).status;
function success(answer) {
	assert.equal(answer.status, SUCCESS, answer.stderr);
	return answer.stdout;
}
const git = (...args) =>
	success(run("git", ["-C", checkout, "-c", "user.name=example", "-c", "user.email=example@example.invalid", ...args])).trim();
function catalog(description) {
	return [
		"authority:",
		"  owner: stado",
		"  rule: test catalog",
		"products:",
		"- id: alpha",
		"  name: alpha",
		"  status: preview",
		"  visibility: private",
		"  family: wisent",
		"  owner_repository: example-org/alpha",
		"  star_repository: example-org/alpha",
		`  description: ${description}`,
		"  roadmap: []",
		"  integrations: []",
		"  evidence:",
		"  - example-org/alpha/README.md",
		"  surfaces:",
		"  - kind: cli",
		"    repository: example-org/alpha",
		"",
	].join("\n");
}
const read = (args = []) => run(stado, ["product", "--workspace", workspace, ...args, "catalog", "--json"]);
const alpha = (answer) => JSON.parse(answer.stdout).products.find((product) => product.id === "alpha");

try {
	mkdirSync(home, { recursive: true });
	mkdirSync(dirname(catalogFile), { recursive: true });
	writeFileSync(catalogFile, catalog("The committed alpha."));
	success(run("git", ["init", "--quiet", "--initial-branch=main", checkout]));
	git("add", "catalog/products.yml");
	git("commit", "--quiet", "-m", "catalog");
	const first = git("rev-parse", "HEAD");

	// 1. An edit that does not validate (the product loses the
	//    star_repository every record names) is not read.
	writeFileSync(catalogFile, catalog("The broken alpha.").replace("  star_repository: example-org/alpha\n", ""));
	const broken = read();
	success(broken);
	assert.equal(alpha(broken).description, "The committed alpha.");
	const where = JSON.parse(broken.stdout).catalog_read;
	assert.equal(where?.revision, first, `catalog_read names the committed head: ${JSON.stringify(where)}`);
	assert.equal(where?.chosen, "committed-head");
	// The notice prints the whole cause chain: the catalog-level refusal,
	// then the product and field it names.
	assert.match(broken.stderr, /uncommitted edit that does not validate: .*alpha names no star_repository.*; it is not read/s);
	const claims = run(stado, ["product", "--workspace", workspace, "registry", "check-claims", "alpha", "--json"]);
	success(claims);
	report.cases.push({ case: "invalid-edit", catalog_read: where, stderr: broken.stderr });

	// 2. A valid edit is not read until it is committed.
	writeFileSync(catalogFile, catalog("The edited alpha."));
	const pending = read();
	success(pending);
	assert.equal(alpha(pending).description, "The committed alpha.");
	assert.match(pending.stderr, /has an uncommitted edit; readers read it once it is committed/);

	// 3. Committed, it is read at the new revision.
	git("commit", "--quiet", "-am", "edit");
	const second = git("rev-parse", "HEAD");
	const committed = read();
	success(committed);
	assert.equal(alpha(committed).description, "The edited alpha.");
	assert.equal(JSON.parse(committed.stdout).catalog_read?.revision, second);
	assert.doesNotMatch(committed.stderr, /uncommitted edit/);
	report.cases.push({ case: "committed-edit", revision: second });

	// 4. An edit through the registry says who sees it.
	const set = JSON.parse(success(run(stado,
		["product", "--workspace", workspace, "registry", "set", "alpha", "--description", "The set alpha.", "--json"])));
	assert.equal(set.uncommitted?.path, "catalog/products.yml", JSON.stringify(set));
	assert.match(set.uncommitted?.commit ?? "", /^git -C .+ commit -m <message> -- catalog\/products\.yml$/);
	assert.match(readFileSync(catalogFile, "utf8"), /The set alpha\./);
	assert.equal(alpha(read()).description, "The edited alpha.");
	report.cases.push({ case: "registry-edit", answer: set.uncommitted });

	// 5. An untracked catalog is read as it stands.
	const loose = join(home, "products.yml");
	writeFileSync(loose, catalog("The loose alpha."));
	const untracked = read(["--catalog", loose]);
	success(untracked);
	assert.equal(alpha(untracked).description, "The loose alpha.");
	assert.equal(JSON.parse(untracked.stdout).catalog_read, null);
	report.cases.push({ case: "untracked" });
	report.result = "passed";
} finally {
	report.finished_at = new Date().toISOString();
	writeFileSync(join(output, "report.json"), JSON.stringify(report, undefined, "\t"));
	console.error(`report: ${join(output, "report.json")}`);
	if (report.result === "passed") {
		rmSync(workspace, { recursive: true, force: true });
		rmSync(home, { recursive: true, force: true });
	}
}
