import assert from "node:assert/strict";
import { join } from "node:path";

import { journey } from "./fixture.mjs";

// Real qualification of `stado product dependencies` over a checkout that
// holds a git submodule: git lists the submodule as a tracked path, but in the
// working tree it is a directory holding another repository's commit, not a
// file of the product. The Stado under test reads the product's own files,
// reports what they reach, and does not read the submodule as its code: a call
// inside the submodule's repository is not the product's dependency. Required:
// STADO_BIN, the Stado executable under test.
await journey(
	"submodule",
	"A real git checkout with a submodule read by the Stado under test against a catalog file",
	import.meta.url,
	({ SUCCESS, run, success, commit, checkout, catalog, dependencies, reached }) => {
		const vendored = checkout("vendored", {
			"src/main.rs":
				'fn main() {\n    let _ = std::process::Command::new("gamma").status();\n}\n',
		});
		const alpha = checkout("alpha", {
			"src/main.rs":
				'fn main() {\n    let _ = std::process::Command::new("beta").status();\n}\n',
			"src/standalone.rs": "pub fn without_beta() {}\n",
		});
		success(
			run("git", [
				"-c",
				"protocol.file.allow=always",
				"-C",
				alpha,
				"submodule",
				"add",
				"--quiet",
				vendored,
				"vendor/vendored",
			]),
		);
		commit(alpha, "Vendor a submodule");
		checkout("beta", { "src/main.rs": "fn main() {}\n" });
		checkout("gamma", { "src/main.rs": "fn main() {}\n" });
		catalog([
			{
				product: "beta",
				surface: "cli",
				alternative: "alpha runs its own copy",
				alternative_source: "alpha/src/standalone.rs",
			},
		]);

		const found = dependencies("alpha");
		assert.equal(found.answer.status, SUCCESS, found.answer.stderr);
		assert.equal(found.report.independent, true);
		assert.equal(reached(found, "beta").standing, "independent");
		assert.equal(
			found.report.dependencies.some(
				(dependency) => dependency.product === "gamma",
			),
			false,
			`the submodule at ${join(alpha, "vendor/vendored")} was read as alpha's code`,
		);
		assert.ok(
			reached(found, "beta").sites.every(
				(site) => !site.file.startsWith("vendor/"),
			),
		);
	},
);
