import assert from "node:assert/strict";

import { journey } from "./fixture.mjs";

// Real qualification of `stado product dependencies`: the Stado under test
// reads real git checkouts of an isolated workspace against a catalog file,
// and its answer, refusals and exit status are checked for every standing a
// product the code reaches can have: undeclared, bound, unproven,
// source-missing and independent, plus an absent checkout and an unknown
// product. A comment line, a file under tests/, the forge's automation, git's
// metadata, an example file, a lockfile and a manifest's test target that name
// the other product are not dependencies. Required: STADO_BIN, the Stado
// executable under test.
await journey(
	"independence",
	"Real git checkouts of an isolated workspace read by the Stado under test against a catalog file, for every dependency standing",
	import.meta.url,
	({ SUCCESS, checkout, catalog, dependencies, reached, invoke }) => {
		checkout("alpha", {
			"src/main.rs":
				'fn main() {\n    let _ = std::process::Command::new("beta").status();\n}\n',
			"src/standalone.rs": "pub fn without_beta() {}\n",
			"src/notes.rs":
				'// std::process::Command::new("gamma") is how gamma would be called\n',
			"tests/calls.rs":
				'fn t() { let _ = std::process::Command::new("gamma").status(); }\n',
			// Files the product never runs: the forge's automation, git's
			// metadata, an example to copy, a lockfile and a manifest's test
			// target each name gamma without alpha calling it.
			".github/workflows/ci.yml": "env:\n  GAMMA_TOKEN: example\n",
			".gitignore": ".gamma/\n",
			".env.example": "GAMMA_URL=\n",
			"package-lock.json": '{"packages": {"node_modules/gamma": {}}}\n',
			"Package.swift":
				'let package = [.testTarget(name: "AlphaTests", path: "tests/gamma")]\n',
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
			"a file the product never runs was read as a dependency",
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
			{
				product: "beta",
				surface: "cli",
				alternative: "alpha runs its own copy",
			},
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
		assert.ok(
			found.report.repositories.every((repository) => !repository.read),
		);
		assert.match(
			found.answer.stderr,
			/gamma: checkout .*gamma of example-org\/gamma is absent, so what it calls is unknown/,
		);

		const unknown = invoke(["dependencies", "delta"]);
		assert.notEqual(unknown.status, SUCCESS);
		assert.match(unknown.stderr, /names no product delta/);
	},
);
