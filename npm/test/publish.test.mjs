import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { startRegistry } from "./registry.mjs";

const ROOT = resolve(import.meta.dirname, "../..");
const VERSION = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(/^version = "(.+)"$/m)[1];
const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "linux-x64": "x86_64-unknown-linux-gnu",
};
const PLATFORMS = Object.keys(TARGETS).map((p) => `@havella/commentbuster-${p}`);
const HOST = `${process.platform}-${process.arch}`;

let work;
let registry;
let env;

before(async () => {
  work = mkdtempSync(join(tmpdir(), "commentbuster-npm-"));
  registry = await startRegistry();
  const npmrc = join(work, "npmrc");
  writeFileSync(
    npmrc,
    `registry=${registry.url}/\n//${new URL(registry.url).host}/:_authToken=test\ncache=${join(work, "cache")}\n` +
      "audit=false\nfund=false\nupdate-notifier=false\nfetch-retries=0\n",
  );
  env = { ...process.env, npm_config_userconfig: npmrc };
  for (const name of Object.keys(env)) {
    if (name === "GITHUB_ACTIONS" || (/^npm_config_/i.test(name) && name !== "npm_config_userconfig")) delete env[name];
  }
});

after(async () => {
  await registry.close();
  rmSync(work, { recursive: true, force: true });
});

// The registry runs in this process, so children must not block it.
function run(command, args, { input, ...options } = {}) {
  return new Promise((done) => {
    const child = execFile(command, args, { env, cwd: work, ...options }, (error, stdout, stderr) =>
      done({ code: error ? error.code : 0, signal: error?.signal, stdout, stderr }),
    );
    child.stdin.on("error", () => {});
    child.stdin.end(input ?? "");
  });
}

const publish = (...args) => run(process.execPath, [join(ROOT, "npm/publish.mjs"), ...args]);

/// A release directory whose binaries are shell scripts.
function release(name, { version = VERSION, script } = {}) {
  const dir = join(work, name);
  mkdirSync(dir, { recursive: true });
  const sums = [];
  for (const target of Object.values(TARGETS)) {
    const folder = `commentbuster-${target}`;
    mkdirSync(join(dir, folder));
    const body = script ?? `if [ "$1" = --version ]; then echo "commentbuster ${version}"; exit 0; fi\necho ${target}`;
    writeFileSync(join(dir, folder, "commentbuster"), `#!/bin/sh\n${body}\n`);
    chmodSync(join(dir, folder, "commentbuster"), 0o755);
    execFileSync("tar", ["-czf", `${folder}.tar.gz`, folder], { cwd: dir });
    rmSync(join(dir, folder), { recursive: true });
    const sha = createHash("sha256").update(readFileSync(join(dir, `${folder}.tar.gz`))).digest("hex");
    sums.push(`${sha}  ${folder}.tar.gz`);
  }
  writeFileSync(join(dir, "SHA256SUMS"), sums.join("\n") + "\n");
  return dir;
}

const manifest = (dir, name) => JSON.parse(readFileSync(join(dir, name.split("/")[1], "package.json"), "utf8"));

test("a dry run builds every package and publishes nothing", async () => {
  const out = join(work, "out");
  const r = await publish(release("dry"), "--dry-run", "--out", out);
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(registry.published, []);

  for (const [platform, target] of Object.entries(TARGETS)) {
    const name = `@havella/commentbuster-${platform}`;
    const pkg = manifest(out, name);
    const [os, cpu] = platform.split("-");
    assert.equal(pkg.name, name);
    assert.equal(pkg.version, VERSION);
    assert.deepEqual(pkg.os, [os]);
    assert.deepEqual(pkg.cpu, [cpu]);
    const binary = join(out, `commentbuster-${platform}`, "bin/commentbuster");
    assert.equal(statSync(binary).mode & 0o777, 0o755);
    assert.match(readFileSync(binary, "utf8"), new RegExp(target));
    assert.ok(existsSync(join(out, `commentbuster-${platform}`, "LICENSE")));
  }

  const main = manifest(out, "@havella/commentbuster");
  assert.deepEqual(main.bin, { commentbuster: "bin/commentbuster.js" });
  assert.deepEqual(main.optionalDependencies, Object.fromEntries(PLATFORMS.map((p) => [p, VERSION])));
  assert.equal(main.version, VERSION);

  for (const name of ["@havella/commentbuster", ...PLATFORMS]) {
    const pkg = manifest(out, name);
    assert.equal(pkg.scripts, undefined, "no install scripts");
    assert.equal(pkg.publishConfig.access, "public");
    assert.equal(pkg.repository.url, "git+https://github.com/NoahDavey/commentbuster.git");
  }
});

test("publishing goes platforms first, and a second run skips everything", async () => {
  const dir = release("real");
  const r = await publish(dir);
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(registry.published.map((p) => p.name), [...PLATFORMS, "@havella/commentbuster"]);
  assert.ok(registry.published.every((p) => p.access === "public" && p.version === VERSION));

  const again = await publish(dir);
  assert.equal(again.code, 0, again.stderr);
  assert.equal(registry.published.length, 5);
  assert.equal(again.stdout.match(/already on npm/g).length, 5);
});

test("a run that stopped part way is resumed", async () => {
  const removed = ["@havella/commentbuster-linux-x64", "@havella/commentbuster"];
  for (const name of removed) registry.packages.delete(name);
  const before = registry.published.length;
  const r = await publish(join(work, "real"));
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(registry.published.slice(before).map((p) => p.name), removed);
});

test("a new version of a package that exists is published", async () => {
  const name = "@havella/commentbuster-linux-x64";
  const known = registry.packages.get(name);
  known.versions = { "0.0.1": { ...known.versions[VERSION], version: "0.0.1" } };
  known["dist-tags"].latest = "0.0.1";
  const before = registry.published.length;
  const r = await publish(join(work, "real"));
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(registry.published.slice(before), [{ name, version: VERSION, access: "public" }]);
});

test("a registry that cannot be read stops the run", async () => {
  const before = registry.published.length;
  registry.failNextWith(503);
  const r = await publish(join(work, "real"));
  registry.failNextWith(0);
  assert.notEqual(r.code, 0);
  assert.match(r.stderr, /could not check/);
  assert.equal(registry.published.length, before);
});

test("a tarball that does not match SHA256SUMS is refused", async () => {
  const dir = release("tampered");
  writeFileSync(join(dir, `commentbuster-${TARGETS["linux-x64"]}.tar.gz`), "other");
  const r = await publish(dir, "--dry-run");
  assert.notEqual(r.code, 0);
  assert.match(r.stderr, /checksum mismatch.*x86_64-unknown-linux-gnu/);
});

test("a missing tarball or SHA256SUMS is refused", async () => {
  const dir = release("partial");
  rmSync(join(dir, `commentbuster-${TARGETS["darwin-x64"]}.tar.gz`));
  assert.match((await publish(dir, "--dry-run")).stderr, /x86_64-apple-darwin/);
  rmSync(join(dir, "SHA256SUMS"));
  assert.match((await publish(dir, "--dry-run")).stderr, /SHA256SUMS/);
  assert.notEqual((await publish(dir, "--dry-run")).code, 0);
});

test("binaries from another release are refused", async () => {
  const r = await publish(release("stale", { version: "0.0.1" }), "--dry-run");
  assert.notEqual(r.code, 0);
  assert.match(r.stderr, new RegExp(`0\\.0\\.1.*${VERSION}`));
});

test("usage errors are reported", async () => {
  assert.notEqual((await publish()).code, 0);
  assert.notEqual((await publish("a", "b")).code, 0);
  assert.notEqual((await publish("a", "--nope")).code, 0);
});

// --- The launcher, as npm installs it ---------------------------------------

/// Install from the registry with scripts disabled.
async function install(name) {
  const dir = join(work, name);
  mkdirSync(dir);
  writeFileSync(join(dir, "package.json"), "{}");
  const r = await run("npm", ["install", "--ignore-scripts", `@havella/commentbuster@${VERSION}`], { cwd: dir });
  assert.equal(r.code, 0, r.stderr);
  return dir;
}

const launcher = (dir) => join(dir, "node_modules/.bin/commentbuster");
const platformDir = (dir) => join(dir, "node_modules/@havella", `commentbuster-${HOST}`);

/// Replace the installed binary's script.
function binary(dir, script) {
  writeFileSync(join(platformDir(dir), "bin/commentbuster"), `#!/bin/sh\n${script}\n`);
}

test("only the host's platform package is installed, with its binary executable", async () => {
  const dir = await install("install");
  for (const platform of Object.keys(TARGETS)) {
    const path = join(dir, "node_modules/@havella", `commentbuster-${platform}`, "bin/commentbuster");
    assert.equal(existsSync(path), platform === HOST, platform);
  }
  assert.ok(statSync(join(platformDir(dir), "bin/commentbuster")).mode & 0o111);
  const r = await run(launcher(dir), []);
  assert.deepEqual([r.code, r.stdout], [0, `${TARGETS[HOST]}\n`]);
});

test("stdio, arguments and the exit code pass through", async () => {
  const dir = await install("passthrough");
  binary(dir, 'echo "args:$#:$1:$2"; cat; echo problem >&2; exit 2');
  const r = await run(launcher(dir), ["claude-gate", "a b"], { input: "payload é\n" });
  assert.deepEqual(r, { code: 2, signal: null, stdout: "args:2:claude-gate:a b\npayload é\n", stderr: "problem\n" });
  for (const code of [0, 1, 42]) {
    binary(dir, `exit ${code}`);
    assert.equal((await run(launcher(dir), [])).code, code);
  }
});

test("a binary killed by a signal is a failure, never exit 2", async () => {
  const dir = await install("signal");
  binary(dir, "kill -KILL $$");
  assert.equal((await run(launcher(dir), [])).code, 137);
});

test("a missing binary exits 1 and says what to do", async () => {
  const dir = await install("missing");
  rmSync(platformDir(dir), { recursive: true });
  const r = await run(launcher(dir), ["claude-gate"], { input: "{}" });
  assert.deepEqual([r.code, r.stdout], [1, ""]);
  assert.match(r.stderr, new RegExp(`@havella/commentbuster-${HOST} is not installed`));
  assert.match(r.stderr, /optional dependencies/);
});

test("a binary that cannot be run exits 1", async () => {
  const dir = await install("unrunnable");
  chmodSync(join(platformDir(dir), "bin/commentbuster"), 0o644);
  const r = await run(launcher(dir), []);
  assert.deepEqual([r.code, r.stdout], [1, ""]);
  assert.match(r.stderr, /could not run .*EACCES/);

  writeFileSync(join(platformDir(dir), "bin/commentbuster"), "#!/no/such/interpreter\n");
  chmodSync(join(platformDir(dir), "bin/commentbuster"), 0o755);
  const missing = await run(launcher(dir), []);
  assert.equal(missing.code, 1);
  assert.match(missing.stderr, /could not run .*ENOENT/);
});

test("an unsupported platform exits 1 and lists the supported ones", async () => {
  const dir = await install("unsupported");
  const preload = join(work, "platform.cjs");
  writeFileSync(preload, 'Object.defineProperty(process, "platform", { value: "win32" });');
  const r = await run(process.execPath, ["--require", preload, launcher(dir)]);
  assert.deepEqual([r.code, r.stdout], [1, ""]);
  assert.match(r.stderr, new RegExp(`win32-${process.arch} is not supported`));
  assert.match(r.stderr, /darwin-arm64, darwin-x64, linux-arm64, linux-x64/);
});
