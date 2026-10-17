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

// The isolated workspace the `stado product dependencies` journeys read: real
// git checkouts below this checkout's ignored .build, a catalog file of three
// fictional products, and the Stado under test named by STADO_BIN. Each
// journey runs inside `journey`, which records every command, the source
// revision and the result in report.json and removes the workspace at the end.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
// The schema version the repository's own catalog declares.
const schema = Number(
	/^schema_version:\s*(?<version>\S+)/m.exec(
		readFileSync(join(root, "catalog/products.yml"), "utf8"),
	).groups.version,
);

export async function journey(name, scope, testFile, body) {
	mkdirSync(join(root, ".build"), { recursive: true });
	const output = mkdtempSync(
		join(realpathSync(join(root, ".build")), `dependencies-${name}-`),
	);
	const home = join(output, "home");
	const workspace = join(home, "workspace");
	let catalogPath = join(output, "products.yml");
	const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace };
	const report = {
		started_at: new Date().toISOString(),
		commands: [],
		observations: [],
		result: "failed",
	};
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
	function record(id, dependencies, installations) {
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
			...(installations ? { installations } : {}),
			...(dependencies ? { dependencies } : {}),
		};
	}
	// alpha's one installation declares the release manifest it is built
	// from, which the instrument reads as how alpha is built, not what it runs.
	const alphaInstallations = [
		{
			surface: "cli",
			kind: "stado-release",
			repository: "example-org/alpha",
			manifest: ".wisent-release.json",
		},
	];
	function catalog(alphaDependencies, declares = {}) {
		// JSON is YAML, so the catalog file is written as JSON. `declares`
		// adds catalog-level declarations, such as the environment the real
		// catalog names for each product.
		writeFileSync(
			catalogPath,
			JSON.stringify({
				schema_version: schema,
				authority: { owner: "stado", rule: "test catalog" },
				...declares,
				products: [
					record("alpha", alphaDependencies, alphaInstallations),
					record("beta"),
					record("gamma"),
				],
			}),
		);
	}
	// Moves the catalog into the checkout `name` at `file`, as the real
	// catalog lives in Stado's own repository; later invocations read it there.
	function catalogIn(name, file) {
		catalogPath = join(workspace, name, file);
		mkdirSync(dirname(catalogPath), { recursive: true });
	}
	function commit(path, message) {
		// The operator's global Git configuration, its hooks path included,
		// never judges the fixture's own history.
		success(
			run(
				"git",
				[
					"-C",
					path,
					"-c",
					"user.name=Dependencies test",
					"-c",
					"user.email=deps@example.invalid",
					"commit",
					"--quiet",
					"-m",
					message,
				],
				{ ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" },
			),
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
		commit(path, "Example");
		return path;
	}
	function invoke(args) {
		return run(
			stado,
			["product", "--workspace", workspace, "--catalog", catalogPath, ...args],
			isolated,
		);
	}
	function dependencies(product) {
		const answer = invoke(["dependencies", product, "--json"]);
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
		report.test_sha256 = digest(readFileSync(fileURLToPath(testFile)));
		report.stado = {
			path: stado,
			sha256: digest(readFileSync(stado)),
			version: success(run(stado, ["--version"])),
		};
		await body({
			SUCCESS,
			workspace,
			run,
			success,
			catalog,
			catalogIn,
			commit,
			checkout,
			invoke,
			dependencies,
			reached,
		});
		report.result = "passed";
	} catch (error) {
		report.error = error.stack || String(error);
		throw error;
	} finally {
		rmSync(home, { recursive: true, force: true });
		report.finished_at = new Date().toISOString();
		report.scope = scope;
		writeFileSync(
			join(output, "report.json"),
			`${JSON.stringify(report, null, 2)}\n`,
			{ mode: constants.S_IRUSR | constants.S_IWUSR },
		);
		console.log(`${report.result}: ${join(output, "report.json")}`);
	}
}
