#!/usr/bin/env node
// Build the npm packages from a release's tarballs and publish them.
//
//   gh release download v1.2.3 --dir release
//   node npm/publish.mjs release [--dry-run] [--out <dir>]
//
// The version is Cargo.toml's. Platform packages go first, so the main package
// is never installable without them, and versions already on npm are skipped,
// so a run that stopped part way can be repeated.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const ROOT = resolve(import.meta.dirname, "..");
const MAIN = "@havella/commentbuster";
const PLATFORMS = [
  { os: "darwin", cpu: "arm64", target: "aarch64-apple-darwin" },
  { os: "darwin", cpu: "x64", target: "x86_64-apple-darwin" },
  { os: "linux", cpu: "arm64", target: "aarch64-unknown-linux-gnu" },
  { os: "linux", cpu: "x64", target: "x86_64-unknown-linux-gnu" },
].map((p) => ({ ...p, name: `${MAIN}-${p.os}-${p.cpu}`, tarball: `commentbuster-${p.target}.tar.gz` }));

function fail(message) {
  console.error(`publish.mjs: ${message}`);
  process.exit(1);
}

function cargo(field) {
  const value = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(new RegExp(`^${field} = "(.+)"$`, "m"));
  return value?.[1] ?? fail(`no ${field} in Cargo.toml`);
}

function verifyChecksums(release) {
  let sums;
  try {
    sums = readFileSync(join(release, "SHA256SUMS"), "utf8");
  } catch {
    fail(`no SHA256SUMS in ${release}`);
  }
  for (const { tarball } of PLATFORMS) {
    const expected = sums.match(new RegExp(`^([0-9a-f]{64})\\s+\\*?(?:\\./)?${tarball}$`, "m"))?.[1];
    if (!expected) fail(`no entry for ${tarball} in SHA256SUMS`);
    let content;
    try {
      content = readFileSync(join(release, tarball));
    } catch {
      fail(`no ${tarball} in ${release}`);
    }
    if (createHash("sha256").update(content).digest("hex") !== expected) {
      fail(`checksum mismatch for ${tarball}`);
    }
  }
}

function writePackage(dir, manifest, readme) {
  mkdirSync(join(dir, "bin"), { recursive: true });
  const common = {
    version,
    license: cargo("license"),
    repository: { type: "git", url: "git+https://github.com/NoahDavey/commentbuster.git" },
    files: ["bin"],
    publishConfig: { access: "public" },
  };
  const { name, description, ...rest } = manifest;
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({ name, description, ...common, ...rest }, null, 2) + "\n",
  );
  copyFileSync(join(ROOT, "LICENSE"), join(dir, "LICENSE"));
  writeFileSync(join(dir, "README.md"), readme);
}

function buildPlatform(release, out, { name, os, cpu, target, tarball }) {
  const dir = join(out, name.split("/")[1]);
  writePackage(
    dir,
    {
      name,
      description: `The commentbuster binary for ${os} ${cpu}`,
      os: [os],
      cpu: [cpu],
      // Yarn would otherwise leave the binary inside a zip.
      preferUnplugged: true,
    },
    `# ${name}\n\nThe ${os} ${cpu} binary for [${MAIN}](https://www.npmjs.com/package/${MAIN}). Install that instead.\n`,
  );
  const extracted = mkdtempSync(join(tmpdir(), "commentbuster-tar-"));
  execFileSync("tar", ["-xzf", join(release, tarball), "-C", extracted]);
  const binary = join(dir, "bin/commentbuster");
  copyFileSync(join(extracted, `commentbuster-${target}`, "commentbuster"), binary);
  chmodSync(binary, 0o755);
  rmSync(extracted, { recursive: true });
  return dir;
}

function buildMain(out) {
  const dir = join(out, MAIN.split("/")[1]);
  writePackage(
    dir,
    {
      name: MAIN,
      description: cargo("description"),
      bin: { commentbuster: "bin/commentbuster.js" },
      optionalDependencies: Object.fromEntries(PLATFORMS.map((p) => [p.name, version])),
    },
    readFileSync(join(ROOT, "README.md"), "utf8"),
  );
  const launcher = join(dir, "bin/commentbuster.js");
  copyFileSync(join(ROOT, "npm/launcher.js"), launcher);
  chmodSync(launcher, 0o755);
  return dir;
}

// Tarball names carry no version, so an old download looks like a new one.
function verifyVersion(dirs) {
  const host = PLATFORMS.findIndex((p) => p.os === process.platform && p.cpu === process.arch);
  if (host < 0) {
    console.warn(`cannot run a ${process.platform}-${process.arch} binary; its version is unchecked`);
    return;
  }
  const reported = execFileSync(join(dirs[host], "bin/commentbuster"), ["--version"], { encoding: "utf8" }).trim();
  if (reported !== `commentbuster ${version}`) {
    fail(`the release holds "${reported}" but Cargo.toml is at ${version}`);
  }
}

function isPublished(name) {
  const args = ["view", `${name}@${version}`, "version", "--json", "--prefer-online"];
  const result = spawnSync("npm", args, { encoding: "utf8" });
  if (result.status === 0) return result.stdout.trim() !== "";
  let code;
  try {
    code = JSON.parse(result.stdout).error.code;
  } catch {
    // Not npm's JSON error; reported below.
  }
  if (code === "E404") return false;
  return fail(`could not check whether ${name}@${version} is on npm:\n${result.stderr}`);
}

let args;
try {
  args = parseArgs({
    allowPositionals: true,
    options: { "dry-run": { type: "boolean", default: false }, out: { type: "string" } },
  });
  if (args.positionals.length !== 1) throw new Error("expected one release directory");
} catch (e) {
  fail(`${e.message}\nusage: node npm/publish.mjs <release-dir> [--dry-run] [--out <dir>]`);
}
const release = resolve(args.positionals[0]);
const dryRun = args.values["dry-run"];
const version = cargo("version");

verifyChecksums(release);
const out = args.values.out ? resolve(args.values.out) : mkdtempSync(join(tmpdir(), "commentbuster-npm-"));
if (!args.values.out) process.on("exit", () => rmSync(out, { recursive: true, force: true }));
const dirs = PLATFORMS.map((p) => buildPlatform(release, out, p));
verifyVersion(dirs);
dirs.push(buildMain(out));
console.log(args.values.out ? `built ${version} in ${out}` : `built ${version}`);

const publishArgs = ["--access", "public"];
if (dryRun) publishArgs.push("--dry-run");
// Provenance needs the workflow's OIDC identity.
else if (process.env.GITHUB_ACTIONS) publishArgs.push("--provenance");

for (const [i, name] of [...PLATFORMS.map((p) => p.name), MAIN].entries()) {
  if (isPublished(name)) {
    console.log(`${name}@${version} is already on npm, skipping`);
    continue;
  }
  const result = spawnSync("npm", ["publish", dirs[i], ...publishArgs], { stdio: "inherit" });
  if (result.status !== 0) fail(`npm publish failed for ${name}@${version}`);
}
