import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	constants,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real qualification of `stado product name`: the Stado under test decides
// product names in an isolated catalog file of two fictional products, alpha
// and beta, where beta integrates with and depends on alpha, alpha declares a
// service and a stado-release installation, and an isolated home holds an
// installation receipt of alpha's cli surface. After every command the
// catalog file and the receipts are read back.
//
// `name set alpha --id gamma --name Gamma` must record the decision (who,
// when, why) as gamma's `naming`, keep alpha (alpha) as a former name, carry
// beta's integration and dependency to gamma, keep the service label,
// directory key and release name alpha's hosts and releases still answer to,
// move the receipt to gamma and name the release manifest the workspace
// checkout still declares as alpha. `name show alpha` must answer gamma,
// `name check` must call alpha and Gamma taken and a new word free, and
// `name forget` must remove a former name recorded in error. Every refusal is
// asserted with its sentence and leaves the catalog as it was: a set without
// a decision maker or reason, one that changes nothing, a name another
// product holds (current or former), an id that cannot be one, a former name
// given as the product, an unknown product, forgetting a current name, and
// `registry set --name`, which no longer exists.
//
// Required: STADO_BIN, the Stado executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "product-name-decisions-"));
const home = join(output, "home");
const workspace = join(output, "workspace");
const catalogPath = join(output, "products.yml");
const report = { started_at: new Date().toISOString(), commands: [], result: "failed" };
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
function run(program, args, environment = process.env) {
	const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: "utf8" });
	report.commands.push({ program, arguments: args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
	return answer;
}
const SUCCESS = run("/usr/bin/true", []).status;
const FAILURE = run("/usr/bin/false", []).status;
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace };
const name = (...args) => run(stado, ["product", "--catalog", catalogPath, "name", ...args], isolated);
const catalog = () => {
	const answer = run(stado, ["product", "--catalog", catalogPath, "catalog", "--json"], isolated);
	assert.equal(answer.status, SUCCESS, answer.stderr);
	return Object.fromEntries(JSON.parse(answer.stdout).products.map((product) => [product.id, product]));
};
function refused(args, sentence, before) {
	const answer = name(...args);
	assert.notEqual(answer.status, SUCCESS, `a refused naming passed: ${args.join(" ")}`);
	assert.match(answer.stderr, sentence);
	assert.equal(readFileSync(catalogPath, "utf8"), before, `a refused naming changed the catalog: ${args.join(" ")}`);
}
const alpha = [
	"- id: alpha",
	"  name: alpha",
	"  status: active",
	"  visibility: private",
	"  family: wisent",
	"  owner_repository: example-org/alpha",
	"  star_repository: example-org/alpha",
	"  description: The alpha test product.",
	"  roadmap: []",
	"  integrations: []",
	"  evidence:",
	"  - example-org/alpha/README.md",
	"  surfaces:",
	"  - kind: cli",
	"    repository: example-org/alpha",
	"  - kind: service",
	"    repository: example-org/alpha",
	"  installations:",
	"  - surface: cli",
	"    kind: stado-release",
	"    repository: example-org/alpha",
	"    manifest: .wisent-release.json",
	"  - surface: service",
	"    kind: stado-release",
	"    repository: example-org/alpha",
	"    manifest: .wisent-release.json",
	"  service:",
	"    installable: true",
	"    program: $HOME/.stado/services/alpha/current/$STADO_PLATFORM/bin/alpha",
	"    args: []",
	"    summary: The alpha test service.",
];
const beta = [
	"- id: beta",
	"  name: Beta",
	"  status: preview",
	"  visibility: private",
	"  family: wisent",
	"  owner_repository: example-org/beta",
	"  star_repository: example-org/beta",
	"  description: The beta test product.",
	"  roadmap: []",
	"  integrations:",
	"  - product: alpha",
	"    description: Beta reads alpha's answers.",
	"    source: example-org/beta/README.md",
	"  dependencies:",
	"  - product: alpha",
	"    surface: cli",
	"    for_surface: cli",
	"    alternative: beta answers from its own cache without alpha",
	"  evidence:",
	"  - example-org/beta/README.md",
	"  surfaces:",
	"  - kind: cli",
	"    repository: example-org/beta",
];
try {
	report.source_revision = run("git", ["rev-parse", "HEAD"]).stdout.trim();
	report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
	report.stado = { path: stado, sha256: digest(readFileSync(stado)) };
	mkdirSync(join(home, ".stado/products/alpha"), { recursive: true });
	mkdirSync(join(workspace, "alpha"), { recursive: true });
	writeFileSync(join(workspace, "alpha/.wisent-release.json"), `${JSON.stringify({ schema_version: 1, product: "alpha" })}\n`);
	writeFileSync(
		catalogPath,
		`authority:\n  owner: stado\n  rule: test catalog\n# beta stays byte for byte\nproducts:\n${alpha.join("\n")}\n${beta.join("\n")}\n`,
	);
	const receipt = {
		product: "alpha",
		surface: "cli",
		status: "installed",
		installed_at: "2026-01-01T00:00:00Z",
		recipe: {},
		installed_paths: [],
		backups: [],
		host: null,
		source_revision: null,
		previous: null,
	};
	writeFileSync(join(home, ".stado/products/alpha/cli.json"), `${JSON.stringify(receipt)}\n`);

	let before = readFileSync(catalogPath, "utf8");
	refused(["set", "alpha", "--id", "gamma", "--reason", "clearer"], /--decided-by/, before);
	refused(["set", "alpha", "--id", "gamma", "--decided-by", "operator"], /--reason/, before);
	refused(["set", "alpha", "--decided-by", "operator", "--reason", "clearer"], /names nothing to change: give --id NEW-ID, --name "New Name" or both; nothing was changed/, before);
	refused(["set", "alpha", "--name", "alpha", "--decided-by", "operator", "--reason", "clearer"], /alpha is already called alpha \(alpha\); nothing was changed/, before);
	refused(["set", "alpha", "--name", "BETA", "--decided-by", "operator", "--reason", "clearer"], /--name "BETA" is already the id of beta: a name, current or former, belongs to one product/, before);
	refused(["set", "alpha", "--id", "Bad_Id", "--decided-by", "operator", "--reason", "clearer"], /--id "Bad_Id" cannot be a product id; nothing was changed/, before);
	refused(["set", "delta", "--name", "Delta", "--decided-by", "operator", "--reason", "clearer"], /no product delta in .*`stado product name list` lists every product; nothing was changed/, before);
	const removed = run(stado, ["product", "--catalog", catalogPath, "registry", "set", "alpha", "--name", "Other", "--json"], isolated);
	assert.equal(removed.status, 2, "registry set still takes --name");
	assert.match(removed.stderr, /unexpected argument '--name'/);
	assert.equal(readFileSync(catalogPath, "utf8"), before);

	const reason = "Gamma is the name the operator chose for the alpha product";
	let answer = name("set", "alpha", "--id", "gamma", "--name", "Gamma", "--decided-by", "operator", "--reason", reason, "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	const decision = JSON.parse(answer.stdout);
	assert.equal(decision.uncommitted, null, "an untracked catalog was said to wait for a commit");
	let products = catalog();
	assert.equal(products.alpha, undefined, "the former id still names a record");
	assert.equal(products.gamma.name, "Gamma");
	assert.equal(products.gamma.naming.decided_by, "operator");
	assert.equal(products.gamma.naming.reason, reason);
	assert.ok(Date.parse(products.gamma.naming.decided_at) <= Date.now(), "the decision time is not a past RFC 3339 time");
	assert.deepEqual(products.gamma.former_names, [{ id: "alpha", name: "alpha", until: products.gamma.naming.decided_at }]);
	assert.equal(products.beta.integrations[0].product, "gamma", "beta's integration still names the former id");
	assert.equal(products.beta.dependencies[0].product, "gamma", "beta's dependency still names the former id");
	assert.equal(products.gamma.service.unit, "com.wisent.alpha", "the service label hosts run was not kept");
	assert.equal(products.gamma.service.directory_service, "alpha", "the service directory key was not kept");
	assert.deepEqual(
		products.gamma.installations.map((installation) => installation.release_product),
		["alpha", "alpha"],
		"the release name the published releases carry was not kept",
	);
	assert.equal(
		products.gamma.surfaces.find((surface) => surface.kind === "cli").docs_origin,
		"https://alpha.wisent.com",
		"the site the CLI documentation is published at was not kept",
	);
	assert.ok(readFileSync(catalogPath, "utf8").includes("# beta stays byte for byte\n"), "a comment outside the renamed records was lost");
	assert.equal(existsSync(join(home, ".stado/products/alpha")), false, "the receipt stayed under the former id");
	const moved = JSON.parse(readFileSync(join(home, ".stado/products/gamma/cli.json"), "utf8"));
	assert.equal(moved.product, "gamma", "the moved receipt still names the former id");
	assert.deepEqual(decision.receipts.map((row) => row.surface), ["cli"]);
	assert.ok(
		decision.outside.some((row) => row.what === "repository example-org/alpha" && /gh repo rename gamma --repo example-org\/alpha/.test(row.next)),
		`the repository named for the former id is not named: ${JSON.stringify(decision.outside)}`,
	);
	assert.ok(
		decision.outside.some((row) => row.what.endsWith("alpha/.wisent-release.json") && row.state === "declares the former id as its product"),
		`the release manifest still declaring the former id is not named: ${JSON.stringify(decision.outside)}`,
	);
	assert.ok(
		decision.outside.some((row) => row.what === "documentation site https://alpha.wisent.com"),
		`the documentation site addressed by the former id is not named: ${JSON.stringify(decision.outside)}`,
	);

	answer = name("show", "ALPHA", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual([JSON.parse(answer.stdout).product, JSON.parse(answer.stdout).as], ["gamma", "former id"]);
	answer = name("check", "alpha", "--json");
	assert.equal(answer.status, FAILURE, "a former name was called free");
	assert.deepEqual(JSON.parse(answer.stdout).taken, [{ held_by: "gamma", as: "former id" }]);
	answer = name("check", "omega", "--as-id", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.equal(JSON.parse(answer.stdout).free, true);
	answer = name("list", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(JSON.parse(answer.stdout).products.map((product) => product.id), ["gamma", "beta"]);

	before = readFileSync(catalogPath, "utf8");
	refused(["set", "beta", "--id", "alpha", "--decided-by", "operator", "--reason", "clearer"], /--id "alpha" is already the former id of gamma/, before);
	refused(["set", "alpha", "--name", "Alpha Two", "--decided-by", "operator", "--reason", "clearer"], /alpha is the former id of gamma, not a product id: name the product as gamma; nothing was changed/, before);
	refused(["show", "nothing"], /no product is or was called "nothing"/, before);
	refused(["forget", "gamma", "Gamma"], /"Gamma" is gamma's current name, not a former one/, before);
	refused(["forget", "gamma", "zeta"], /gamma has no former name "zeta"; its former names are alpha \(alpha\); nothing was changed/, before);

	answer = name("set", "gamma", "--name", "Gamma Prime", "--decided-by", "operator", "--reason", "a display name only", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	products = catalog();
	assert.equal(products.gamma.name, "Gamma Prime");
	assert.deepEqual(
		products.gamma.former_names.map((row) => [row.id, row.name, row.decided_by, row.reason]),
		[
			["alpha", "alpha", undefined, undefined],
			["gamma", "Gamma", "operator", reason],
		],
		"a renamed display name did not keep the name before it with the decision it stood under",
	);

	answer = name("forget", "gamma", "alpha", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(catalog().gamma.former_names.map((row) => row.id), ["gamma"]);

	// Another machine still keeping receipts under a former id: adopt moves
	// them, and refuses, with the cause, a former id whose receipts the
	// current id already has.
	answer = name("set", "gamma", "--id", "kappa", "--decided-by", "operator", "--reason", "a second id", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.equal(existsSync(join(home, ".stado/products/kappa/cli.json")), true, "the receipt did not follow the second id");
	mkdirSync(join(home, ".stado/products/gamma"), { recursive: true });
	writeFileSync(join(home, ".stado/products/gamma/desktop.json"), `${JSON.stringify({ ...receipt, product: "gamma", surface: "desktop" })}\n`);
	answer = name("adopt", "--json");
	assert.equal(answer.status, FAILURE, "adopt passed with receipts under both the former and the current id");
	assert.match(JSON.parse(answer.stdout).moved[0].detail, /both hold receipts: the catalog names gamma a former id of kappa/);
	assert.equal(existsSync(join(home, ".stado/products/gamma/desktop.json")), true, "a refused adoption moved a receipt");
	rmSync(join(home, ".stado/products/kappa"), { recursive: true });
	answer = name("adopt", "--json");
	assert.equal(answer.status, SUCCESS, answer.stderr);
	assert.deepEqual(JSON.parse(answer.stdout).moved.map((row) => [row.product, row.surface, row.state]), [["kappa", "desktop", "moved"]]);
	const adopted = JSON.parse(readFileSync(join(home, ".stado/products/kappa/desktop.json"), "utf8"));
	assert.equal(adopted.product, "kappa", "the adopted receipt still names the former id");
	assert.equal(existsSync(join(home, ".stado/products/gamma")), false, "the former id's receipt directory stayed");
	report.result = "passed";
} catch (error) {
	report.error = error.stack || String(error);
	throw error;
} finally {
	rmSync(home, { recursive: true, force: true });
	rmSync(workspace, { recursive: true, force: true });
	report.finished_at = new Date().toISOString();
	writeFileSync(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`, {
		mode: constants.S_IRUSR | constants.S_IWUSR,
	});
	console.log(`${report.result}: ${join(output, "report.json")}`);
}
