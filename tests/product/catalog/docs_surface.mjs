import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pinnedGit } from "../../native/home.mjs";
import { fileURLToPath } from "node:url";

// Real qualification of the `docs` surface kind: a product keeps its
// documentation site in a repository of its own beside its `web`
// application, and a product holds one surface of each kind. The Stado under
// test runs with an isolated HOME and a workspace below this checkout's
// ignored .build whose `stado` checkout commits a catalog of one fictional
// product declaring a cli, a web and a docs surface.
//
// 1. `stado product catalog --json` reads the catalog: the product lists
//    both its web and its docs repositories.
// 2. `surface add PRODUCT --kind docs --repository <the declared one>
//    --plan --json` answers that the product already declares it; before the
//    kind existed `--kind docs` was refused as an unknown kind.
// 3. `surface add` naming another docs repository is refused naming the one
//    the product declares, since a product holds one docs surface.
// 4. A catalog declaring two web surfaces still does not validate.
//
// Required: STADO_BIN, the Stado executable under test. Evidence is kept in
// report.json under .build.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-catalog-docs-surface-"));
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
function catalog(documentation) {
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
		"  description: The alpha product.",
		"  roadmap: []",
		"  integrations: []",
		"  evidence:",
		"  - example-org/alpha/README.md",
		"  surfaces:",
		"  - kind: cli",
		"    repository: example-org/alpha",
		"  - kind: web",
		"    repository: example-org/alpha-web",
		`  - kind: ${documentation}`,
		"    repository: example-org/alpha-docs",
		"",
	].join("\n");
}
const product = (...args) => run(stado, ["product", "--workspace", workspace, ...args]);

try {
	mkdirSync(home, { recursive: true });
	mkdirSync(dirname(catalogFile), { recursive: true });
	writeFileSync(catalogFile, catalog("docs"));
	success(run("git", ["init", "--quiet", "--initial-branch=main", checkout]));
	git("add", "catalog/products.yml");
	git("commit", "--quiet", "-m", "catalog");

	// 1. Both repositories are read.
	const read = JSON.parse(success(product("catalog", "--json")));
	const alpha = read.products.find((record) => record.id === "alpha");
	const surfaces = alpha.surfaces.map((surface) => `${surface.kind}=${surface.repository}`);
	assert.ok(surfaces.includes("web=example-org/alpha-web"), JSON.stringify(surfaces));
	assert.ok(surfaces.includes("docs=example-org/alpha-docs"), JSON.stringify(surfaces));
	report.cases.push({ case: "read", surfaces });

	// 2. The declared docs surface is the product's already.
	const declared = JSON.parse(success(product(
		"surface", "add", "alpha", "--kind", "docs", "--repository", "example-org/alpha-docs", "--plan", "--json")));
	assert.equal(declared.state, "declared", JSON.stringify(declared));
	report.cases.push({ case: "declared", state: declared.state });

	// 3. A second docs repository is refused naming the declared one.
	const second = product("surface", "add", "alpha", "--kind", "docs", "--repository", "example-org/alpha-site", "--plan");
	assert.notEqual(second.status, SUCCESS, second.stdout);
	assert.match(second.stderr, /already declares the docs surface example-org\/alpha-docs; a product has one surface of each kind/);
	report.cases.push({ case: "second-docs", stderr: second.stderr });

	// 4. Two web surfaces still do not validate.
	const loose = join(home, "products.yml");
	writeFileSync(loose, catalog("web"));
	const doubled = product("--catalog", loose, "catalog", "--json");
	assert.notEqual(doubled.status, SUCCESS, doubled.stdout);
	assert.match(doubled.stderr, /alpha: duplicate web surface/);
	report.cases.push({ case: "two-web", stderr: doubled.stderr });
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
