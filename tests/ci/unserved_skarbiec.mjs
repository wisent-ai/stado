import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real qualification of `stado ci status` when this host reaches Skarbiec
// through its resolver's own adapter and the resolver does not serve it: the
// request is refused before anything is sent, naming why, rather than
// standing on a port the resolver holds while it reads its registry store.
//
// Each case runs the Stado under test with a HOME of its own under this
// checkout's ignored .build: `~/.stado/forwards/skarbiec.local` names the
// adapter's loopback address, as `stado service directory publish` writes it,
// and `~/.stado/resolver-state.json` is the state a resolver publishes.
// Nothing listens on that address.
//
// 1. A resolver that is running but holds no listener (starting, reading its
//    registry store): refused with "Skarbiec was not asked … holds no
//    listener: <its reason>".
// 2. A resolver whose publishing process has exited: refused naming that it
//    is not running.
// 3. A resolver that holds its listeners and serves that adapter: the request
//    is sent, and the refusal is that Skarbiec did not answer — not that it was
//    not asked.
// 4. A forward marker whose first line names no address: refused before
//    anything is sent as "this machine has no Skarbiec address", naming the
//    marker file, instead of read as an empty address.
//
// Required: STADO_BIN, the Stado executable under test, and
// STADO_CI_TEST_PRODUCT, a catalogued product. Evidence is kept under .build.
const stado = process.env.STADO_BIN;
assert.ok(stado, "STADO_BIN is required: the Stado executable under test");
const product = process.env.STADO_CI_TEST_PRODUCT;
assert.ok(product, "STADO_CI_TEST_PRODUCT is required: a catalogued product");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
mkdirSync(join(root, ".build"), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, ".build")), "ci-unserved-skarbiec-"));
const report = { started_at: new Date().toISOString(), cases: [], result: "failed" };
const SUCCESS = spawnSync("/usr/bin/true", []).status;
const REGISTRY_READ = "reading this host's registry store before binding any port";

// A loopback address nothing listens on: one the kernel handed out, then closed.
async function closedAddress() {
	const server = createServer();
	await new Promise((ready) => server.listen({ host: "localhost" }, ready));
	const { address, port } = server.address();
	await new Promise((closed) => server.close(closed));
	return { bind: `${address.includes(":") ? `[${address}]` : address}:${port}` };
}

// A pid that no running process holds: a process that already exited.
function exitedPid() {
	const exited = spawnSync("/bin/sh", ["-c", "echo $$"], { encoding: "utf8" });
	assert.equal(exited.status, SUCCESS, exited.stderr);
	return Number(exited.stdout.trim());
}

function home(name, bind, state) {
	const directory = join(output, name);
	mkdirSync(join(directory, ".stado", "forwards"), { recursive: true });
	writeFileSync(join(directory, ".stado", "forwards", "skarbiec.local"), `http://${bind}\n`);
	writeFileSync(join(directory, ".stado", "resolver-state.json"), JSON.stringify(state, null, "\t"));
	return directory;
}

function status(name, directory) {
	const environment = { ...process.env, HOME: directory };
	for (const inherited of [
		"STADO_CONFIG",
		"WC_SKARBIEC_URL",
		"WC_SKARBIEC_CONSUMER",
		"WC_SKARBIEC_TOKEN_FILE",
		"STADO_RESOLVER_STATE_FILE",
		"STADO_CREDENTIALS_STORE",
	]) {
		delete environment[inherited];
	}
	const answer = spawnSync(stado, ["ci", "status", product, "--json"], { cwd: root, env: environment, encoding: "utf8" });
	report.cases.push({
		case: name,
		home: directory,
		program: stado,
		arguments: ["ci", "status", product, "--json"],
		exit_status: answer.status,
		signal: answer.signal,
		error: answer.error?.message,
		stdout: answer.stdout,
		stderr: answer.stderr,
	});
	assert.notEqual(answer.status, SUCCESS, `${name}: ci status succeeded against a Skarbiec nothing serves`);
	return answer.stderr;
}

const published = (pid, state, listening, adapters, reason) => ({
	updated_at: new Date().toISOString(),
	target: "test-host",
	pid,
	state,
	reason,
	listening,
	adapters,
});

try {
	const { bind } = await closedAddress();
	const adapter = { service: "skarbiec", consumer: "stado", bind };

	const starting = status(
		"starting",
		home("starting", bind, published(process.pid, "starting", false, [], REGISTRY_READ)),
	);
	assert.match(starting, /Skarbiec was not asked /, starting);
	assert.ok(starting.includes(`holds no listener: ${REGISTRY_READ}`), starting);
	assert.ok(starting.includes(`pid ${process.pid}`), starting);

	const gone = exitedPid();
	const exited = status("exited", home("exited", bind, published(gone, "serving", true, [adapter], null)));
	assert.match(exited, /Skarbiec was not asked /, exited);
	assert.ok(exited.includes(`(pid ${gone}, state serving at `), exited);
	assert.ok(exited.includes("is not running, and no resolver has published since"), exited);

	const served = status("served", home("served", bind, published(process.pid, "serving", true, [adapter], null)));
	assert.doesNotMatch(served, /Skarbiec was not asked /, served);
	assert.match(served, /Skarbiec did not answer /, served);

	const blank = home("blank-marker", bind, published(process.pid, "serving", true, [adapter], null));
	const marker = join(blank, ".stado", "forwards", "skarbiec.local");
	writeFileSync(marker, "\n");
	const unaddressed = status("blank-marker", blank);
	assert.ok(unaddressed.includes("this machine has no Skarbiec address: "), unaddressed);
	assert.doesNotMatch(unaddressed, /Skarbiec did not answer |invalid Skarbiec URL/, unaddressed);
	assert.ok(unaddressed.includes(`forward marker ${marker} names no address on its first line`), unaddressed);

	report.result = "passed";
} finally {
	report.finished_at = new Date().toISOString();
	writeFileSync(join(output, "report.json"), JSON.stringify(report, null, "\t"));
}
