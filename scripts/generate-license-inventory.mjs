#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const repositoryRoot = path.resolve(scriptDirectory, "..");

function sha256(content) {
  return createHash("sha256").update(content).digest("hex");
}

function npmPackageName(lockPath) {
  const marker = "node_modules/";
  const index = lockPath.lastIndexOf(marker);
  if (index < 0) return undefined;
  const parts = lockPath.slice(index + marker.length).split("/");
  return parts[0]?.startsWith("@") ? `${parts[0]}/${parts[1]}` : parts[0];
}

function requireLicense(ecosystem, name, version, license) {
  if (typeof license !== "string" || license.trim().length === 0) {
    throw new Error(`${ecosystem} package has no declared license: ${name}@${version}`);
  }
  return license;
}

const output = path.resolve(process.argv[2] ?? path.join(repositoryRoot, "release", "THIRD-PARTY-LICENSES.json"));
const cargoLockPath = path.join(repositoryRoot, "Cargo.lock");
const npmLockPath = path.join(repositoryRoot, "apps", "agent-desktop", "package-lock.json");
const packageJsonPath = path.join(repositoryRoot, "apps", "agent-desktop", "package.json");

const [cargoLock, npmLockBytes, packageJsonBytes] = await Promise.all([
  readFile(cargoLockPath),
  readFile(npmLockPath),
  readFile(packageJsonPath),
]);
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  }),
);
const npmLock = JSON.parse(npmLockBytes);
const packageJson = JSON.parse(packageJsonBytes);

const rust = metadata.packages
  .filter((pkg) => pkg.source !== null)
  .map((pkg) => ({
    ecosystem: "cargo",
    name: pkg.name,
    version: pkg.version,
    license: requireLicense("cargo", pkg.name, pkg.version, pkg.license),
    source: pkg.source,
  }));

const npm = Object.entries(npmLock.packages)
  .filter(([lockPath, pkg]) => lockPath !== "" && pkg.link !== true)
  .map(([lockPath, pkg]) => {
    const name = npmPackageName(lockPath);
    if (name === undefined || typeof pkg.version !== "string") {
      throw new Error(`invalid npm lock entry: ${lockPath}`);
    }
    return {
      ecosystem: "npm",
      name,
      version: pkg.version,
      license: requireLicense("npm", name, pkg.version, pkg.license),
      source: pkg.resolved,
    };
  });

const dependencies = [...rust, ...npm].sort((left, right) =>
  `${left.ecosystem}\0${left.name}\0${left.version}`.localeCompare(
    `${right.ecosystem}\0${right.name}\0${right.version}`,
    "en",
  ),
);

const inventory = {
  inventory_version: 1,
  product: "Enterprise Local Agent",
  application_version: packageJson.version,
  generated_from: {
    cargo_lock_sha256: sha256(cargoLock),
    package_lock_sha256: sha256(npmLockBytes),
  },
  counts: {
    cargo_third_party: rust.length,
    npm_third_party: npm.length,
    total: dependencies.length,
    missing_license_declarations: 0,
  },
  dependencies,
};

await writeFile(output, `${JSON.stringify(inventory, null, 2)}\n`, "utf8");
process.stdout.write(`${output}\n`);
