#!/usr/bin/env node
// Runs the commentbuster binary from the platform package npm installed.
//
// Failures here exit 1. Claude Code blocks a tool call on exit 2, so only the
// binary itself may return that.
"use strict";

const { spawnSync } = require("child_process");
const { constants } = require("os");

const scope = "@havella/commentbuster-";
const platform = `${process.platform}-${process.arch}`;
const supported = Object.keys(require("../package.json").optionalDependencies).map((name) =>
  name.slice(scope.length),
);

function fail(message) {
  console.error(`commentbuster: ${message}`);
  process.exit(1);
}

if (!supported.includes(platform)) {
  fail(`${platform} is not supported; there are binaries for ${supported.join(", ")}`);
}

let binary;
try {
  binary = require.resolve(`${scope}${platform}/bin/commentbuster`);
} catch {
  fail(
    `${scope}${platform} is not installed; reinstall @havella/commentbuster with optional dependencies enabled`,
  );
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  // On musl the missing file is the dynamic loader.
  const hint = process.platform === "linux" && result.error.code === "ENOENT" ? " (it needs glibc)" : "";
  fail(`could not run ${binary}: ${result.error.message}${hint}`);
}
process.exit(result.signal ? 128 + constants.signals[result.signal] : result.status);
