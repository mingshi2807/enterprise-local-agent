#!/usr/bin/env node

import { createHash } from "node:crypto";
import { Buffer } from "node:buffer";
import { execFileSync } from "node:child_process";
import { lstat, mkdir, readFile, readdir, readlink, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
export const desktopRoot = path.resolve(scriptDirectory, "..");
export const repositoryRoot = path.resolve(desktopRoot, "../..");

const TARGETS = new Set(["linux-x86_64", "macos-aarch64"]);
const COMPATIBILITY_CONTRACT_FINGERPRINT = "9244db3dd8e2b578ac5b7424c9c5f3f9c29262310db183ccd5c92ba3c434c578";
const REQUIRED_CAPABILITIES = new Set([
  "allow-desktop-build-info",
  "allow-service-health",
  "allow-service-compatibility",
  "allow-service-readiness",
  "allow-service-version",
  "allow-conversation-create-session",
  "allow-conversation-list-sessions",
  "allow-conversation-list-runs",
  "allow-conversation-start-readonly-run",
  "allow-conversation-start-localwrite-run",
  "allow-conversation-run-status",
  "allow-conversation-cancel-run",
  "allow-conversation-read-events",
  "allow-approval-list-waiting",
  "allow-approval-get-preview",
  "allow-approval-submit-decision",
  "allow-approval-resume-run",
  "allow-approval-abort-waiting",
]);

async function readJson(filename) {
  return JSON.parse(await readFile(filename, "utf8"));
}

function workspaceVersion(cargoToml) {
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargoToml)?.[1];
  const version = section === undefined ? undefined : /^version\s*=\s*"([^"]+)"\s*$/m.exec(section)?.[1];
  if (version === undefined) throw new Error("workspace package version is missing");
  return version;
}

function sameMembers(actual, expected) {
  return actual.length === expected.size && actual.every((item) => expected.has(item));
}

function assertRelease(condition, message) {
  if (!condition) throw new Error(message);
}

export async function validateReleaseConfig(root = repositoryRoot) {
  const desktop = path.join(root, "apps/agent-desktop");
  const tauri = path.join(desktop, "src-tauri");
  const [cargo, cargoLock, packageJson, packageLock, config, macConfig, linuxConfig, capabilities, viteConfig, entitlements] =
    await Promise.all([
      readFile(path.join(root, "Cargo.toml"), "utf8"),
      readFile(path.join(root, "Cargo.lock"), "utf8"),
      readJson(path.join(desktop, "package.json")),
      readJson(path.join(desktop, "package-lock.json")),
      readJson(path.join(tauri, "tauri.conf.json")),
      readJson(path.join(tauri, "tauri.macos.conf.json")),
      readJson(path.join(tauri, "tauri.linux.conf.json")),
      readJson(path.join(tauri, "capabilities/main.json")),
      readFile(path.join(desktop, "vite.config.ts"), "utf8"),
      readFile(path.join(tauri, "Entitlements.plist"), "utf8"),
    ]);

  const version = workspaceVersion(cargo);
  assertRelease(packageJson.version === version, "package.json version differs from workspace Cargo version");
  assertRelease(packageLock.version === version, "package-lock.json version differs from workspace Cargo version");
  assertRelease(packageLock.packages?.[""]?.version === version, "package-lock root version differs from workspace Cargo version");
  assertRelease(config.version === undefined, "Tauri must inherit the authoritative Cargo package version");
  assertRelease(config.identifier === "com.enterprise-local-agent.desktop", "unexpected desktop bundle identifier");
  assertRelease(config.bundle?.active === true, "desktop bundling must be active");
  assertRelease(config.bundle?.createUpdaterArtifacts === false, "automatic updater artifacts must remain disabled");
  assertRelease(config.bundle?.externalBin === undefined, "desktop release must not embed runtime sidecars");
  assertRelease(config.bundle?.resources === undefined, "desktop release must not embed unreviewed resources");
  assertRelease(macConfig.bundle?.targets?.join(",") === "app,dmg", "macOS targets must be app and dmg");
  assertRelease(macConfig.bundle?.macOS?.minimumSystemVersion === "12.0", "macOS minimum must be 12.0");
  assertRelease(macConfig.bundle?.macOS?.hardenedRuntime === true, "macOS hardened runtime must be enabled");
  assertRelease(macConfig.bundle?.macOS?.entitlements === "Entitlements.plist", "macOS entitlements file is not pinned");
  assertRelease(linuxConfig.bundle?.targets?.join(",") === "deb", "Linux target must be deb only");
  assertRelease(linuxConfig.productName === "Enterprise Local Agent Desktop", "Linux package name must remain distinct from the service");
  assertRelease(linuxConfig.bundle?.linux?.deb?.section === "devel", "Debian section must be devel");
  assertRelease(linuxConfig.bundle?.linux?.deb?.priority === "optional", "Debian priority must be optional");
  assertRelease(sameMembers(capabilities.permissions ?? [], REQUIRED_CAPABILITIES), "desktop capability allowlist drifted");
  assertRelease(!/(?:shell|http|filesystem|fs:|sql|updater)/i.test((capabilities.permissions ?? []).join("\n")), "desktop capabilities contain a prohibited generic permission");
  const csp = config.app?.security?.csp ?? "";
  assertRelease(csp.includes("default-src 'self'"), "WebView content must remain local");
  assertRelease(csp.includes("connect-src 'none'"), "WebView network access must remain disabled");
  assertRelease(csp.includes("object-src 'none'") && csp.includes("frame-src 'none'"), "active embedded content must remain disabled");
  assertRelease(!/https?:|file:|javascript:/i.test(csp), "CSP must not authorize remote or active URL schemes");
  assertRelease(/sourcemap:\s*false/.test(viteConfig), "production source maps must be disabled");
  assertRelease(/<dict\s*\/>/.test(entitlements), "macOS entitlements must remain empty for M16.9");

  for (const [name, specifier] of Object.entries({ ...packageJson.dependencies, ...packageJson.devDependencies })) {
    assertRelease(/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(specifier), `npm dependency is not exactly pinned: ${name}`);
  }
  for (const [name, locked] of Object.entries(packageLock.packages ?? {})) {
    if (name === "" || locked.link === true) continue;
    assertRelease(typeof locked.integrity === "string" && /^sha512-/.test(locked.integrity), `npm lock integrity is missing: ${name}`);
    assertRelease(
      typeof locked.resolved === "string" && /^https:\/\/registry\.npmjs\.org\//.test(locked.resolved),
      `npm lock source is not the reviewed registry: ${name}`,
    );
  }
  for (const source of cargoLock.matchAll(/^source = "([^"]+)"$/gm)) {
    assertRelease(source[1] === "registry+https://github.com/rust-lang/crates.io-index", `Cargo.lock contains unsupported source: ${source[1]}`);
  }

  for (const icon of config.bundle.icon ?? []) {
    const iconStat = await stat(path.join(tauri, icon));
    assertRelease(iconStat.isFile() && iconStat.size > 0, `bundle icon is missing or empty: ${icon}`);
  }

  return { identifier: config.identifier, version };
}

async function sha256File(filename) {
  const hash = createHash("sha256");
  hash.update(await readFile(filename));
  return hash.digest("hex");
}

async function directoryEntries(root, current = root) {
  const names = (await readdir(current)).sort((left, right) => left.localeCompare(right, "en"));
  const entries = [];
  for (const name of names) {
    const absolute = path.join(current, name);
    const relative = path.relative(root, absolute).split(path.sep).join("/");
    const metadata = await lstat(absolute);
    if (metadata.isDirectory()) {
      entries.push(...(await directoryEntries(root, absolute)));
    } else if (metadata.isFile()) {
      entries.push({ kind: "file", relative, executable: (metadata.mode & 0o111) !== 0, size: metadata.size, digest: await sha256File(absolute) });
    } else if (metadata.isSymbolicLink()) {
      const target = await readlink(absolute);
      const resolved = path.resolve(path.dirname(absolute), target);
      assertRelease(!path.isAbsolute(target) && (resolved === root || resolved.startsWith(`${root}${path.sep}`)), `unsafe symbolic link in artifact: ${relative}`);
      entries.push({
        kind: "symlink",
        relative,
        executable: false,
        size: Buffer.byteLength(target),
        digest: createHash("sha256").update(target).digest("hex"),
      });
    } else {
      throw new Error(`unsupported artifact entry: ${relative}`);
    }
  }
  return entries;
}

async function hashArtifact(filename) {
  const metadata = await lstat(filename);
  if (metadata.isFile()) {
    return { kind: "file", sha256: await sha256File(filename), size_bytes: metadata.size };
  }
  if (!metadata.isDirectory()) throw new Error(`artifact is not a regular file or directory: ${filename}`);
  const entries = await directoryEntries(filename);
  const hash = createHash("sha256");
  let size = 0;
  for (const entry of entries) {
    size += entry.size;
    hash.update(`${entry.kind}\0${entry.relative}\0${entry.executable ? "x" : "-"}\0${entry.size}\0${entry.digest}\0`);
  }
  return { kind: "directory", sha256: hash.digest("hex"), size_bytes: size };
}

function artifactFormat(filename, kind) {
  if (kind === "directory" && filename.endsWith(".app")) return "app";
  if (kind === "file" && filename.endsWith(".dmg")) return "dmg";
  if (kind === "file" && filename.endsWith(".deb")) return "deb";
  throw new Error(`unsupported release artifact: ${filename}`);
}

function git(root, args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

export async function createManifest({
  artifactPaths,
  output,
  root = repositoryRoot,
  sourceDateEpoch = process.env.SOURCE_DATE_EPOCH,
  revision = process.env.ELA_GIT_REVISION,
  target,
  allowDirty = process.env.ELA_RELEASE_ALLOW_DIRTY === "1",
}) {
  assertRelease(TARGETS.has(target), `unsupported release target: ${target}`);
  assertRelease(Array.isArray(artifactPaths) && artifactPaths.length > 0, "at least one artifact is required");
  assertRelease(/^\d+$/.test(sourceDateEpoch ?? ""), "SOURCE_DATE_EPOCH is required and must be an integer");
  const resolvedRevision = revision ?? git(root, ["rev-parse", "HEAD"]);
  assertRelease(/^[0-9a-f]{40}$/.test(resolvedRevision), "git revision must be a lowercase 40-character commit ID");
  const dirty = git(root, ["status", "--porcelain=v1", "--untracked-files=normal"]).length > 0;
  assertRelease(!dirty || allowDirty, "release manifests require a clean worktree");
  const releaseConfig = await validateReleaseConfig(root);

  const artifacts = [];
  const seen = new Set();
  for (const artifactPath of artifactPaths) {
    const absolute = path.resolve(artifactPath);
    const filename = path.basename(absolute);
    assertRelease(filename !== "." && filename !== ".." && !seen.has(filename), `duplicate or invalid artifact filename: ${filename}`);
    seen.add(filename);
    const hashed = await hashArtifact(absolute);
    const format = artifactFormat(filename, hashed.kind);
    assertRelease(target === "linux-x86_64" ? format === "deb" : format === "app" || format === "dmg", `artifact format ${format} does not match ${target}`);
    artifacts.push({ filename, format, ...hashed });
  }
  artifacts.sort((left, right) => left.filename.localeCompare(right.filename, "en"));

  const manifest = {
    manifest_version: 1,
    application: "enterprise-local-agent-desktop",
    application_version: releaseConfig.version,
    bundle_identifier: releaseConfig.identifier,
    target,
    git_revision: resolvedRevision,
    build_profile: "release",
    build_timestamp_utc: new Date(Number(sourceDateEpoch) * 1000).toISOString(),
    source_dirty: dirty,
    service_deployment: "separate",
    automatic_updates: false,
    compatibility: {
      handshake_version: 1,
      service_api_version: 1,
      service_event_version: 2,
      contract_fingerprint: COMPATIBILITY_CONTRACT_FINGERPRINT,
      minimum_macos_version: "12.0",
    },
    artifacts,
  };
  await mkdir(path.dirname(path.resolve(output)), { recursive: true });
  await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`, { flag: "wx" });
  return manifest;
}

function exactKeys(value, expected, label) {
  assertRelease(value !== null && typeof value === "object" && !Array.isArray(value), `${label} must be an object`);
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  assertRelease(JSON.stringify(actual) === JSON.stringify(wanted), `${label} contains unsupported or missing fields`);
}

export async function verifyManifest({ manifestPath, artifactDirectory, root = repositoryRoot, allowDirty = process.env.ELA_RELEASE_ALLOW_DIRTY === "1" }) {
  const manifest = await readJson(manifestPath);
  exactKeys(manifest, ["manifest_version", "application", "application_version", "bundle_identifier", "target", "git_revision", "build_profile", "build_timestamp_utc", "source_dirty", "service_deployment", "automatic_updates", "compatibility", "artifacts"], "manifest");
  exactKeys(manifest.compatibility, ["handshake_version", "service_api_version", "service_event_version", "contract_fingerprint", "minimum_macos_version"], "compatibility");
  assertRelease(manifest.manifest_version === 1, "unsupported release manifest version");
  assertRelease(manifest.application === "enterprise-local-agent-desktop", "unexpected manifest application");
  assertRelease(TARGETS.has(manifest.target), "unsupported manifest target");
  assertRelease(manifest.build_profile === "release", "artifact was not built with the release profile");
  assertRelease(manifest.service_deployment === "separate", "desktop package must not claim an embedded service");
  assertRelease(manifest.automatic_updates === false, "automatic updates are not approved");
  assertRelease(manifest.source_dirty === false || allowDirty, "dirty-source manifest is not releasable");
  assertRelease(manifest.compatibility.handshake_version === 1, "unsupported compatibility handshake version");
  assertRelease(manifest.compatibility.service_api_version === 1 && manifest.compatibility.service_event_version === 2, "unsupported service compatibility metadata");
  assertRelease(manifest.compatibility.contract_fingerprint === COMPATIBILITY_CONTRACT_FINGERPRINT, "unexpected compatibility contract fingerprint");
  assertRelease(manifest.compatibility.minimum_macos_version === "12.0", "unexpected macOS compatibility metadata");
  const releaseConfig = await validateReleaseConfig(root);
  assertRelease(manifest.application_version === releaseConfig.version, "manifest version differs from release configuration");
  assertRelease(manifest.bundle_identifier === releaseConfig.identifier, "manifest bundle identifier differs from release configuration");
  assertRelease(Array.isArray(manifest.artifacts) && manifest.artifacts.length > 0, "manifest has no artifacts");

  const seen = new Set();
  for (const artifact of manifest.artifacts) {
    exactKeys(artifact, ["filename", "format", "kind", "sha256", "size_bytes"], "artifact");
    assertRelease(path.basename(artifact.filename) === artifact.filename && !seen.has(artifact.filename), "artifact filename is unsafe or duplicated");
    seen.add(artifact.filename);
    assertRelease(/^[0-9a-f]{64}$/.test(artifact.sha256), "artifact checksum is invalid");
    const actual = await hashArtifact(path.join(artifactDirectory, artifact.filename));
    assertRelease(actual.kind === artifact.kind && actual.size_bytes === artifact.size_bytes && actual.sha256 === artifact.sha256, `artifact verification failed: ${artifact.filename}`);
  }
  return manifest;
}

function option(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || index + 1 >= args.length) throw new Error(`missing ${name}`);
  const value = args[index + 1];
  args.splice(index, 2);
  return value;
}

async function main() {
  const [command, ...input] = process.argv.slice(2);
  if (command === "validate" && input.length === 0) {
    const result = await validateReleaseConfig();
    process.stdout.write(`release configuration valid: ${result.version} ${result.identifier}\n`);
    return;
  }
  if (command === "manifest") {
    const args = [...input];
    const target = option(args, "--target");
    const output = option(args, "--output");
    assertRelease(args.length > 0 && args.every((item) => !item.startsWith("--")), "manifest requires artifact paths");
    await createManifest({ artifactPaths: args, output, target });
    process.stdout.write(`release manifest written: ${output}\n`);
    return;
  }
  if (command === "verify") {
    const args = [...input];
    const manifestPath = option(args, "--manifest");
    const artifactDirectory = option(args, "--artifact-dir");
    assertRelease(args.length === 0, "unexpected verify arguments");
    await verifyManifest({ manifestPath, artifactDirectory });
    process.stdout.write(`release manifest verified: ${manifestPath}\n`);
    return;
  }
  throw new Error("usage: desktop-release.mjs validate | manifest --target TARGET --output FILE ARTIFACT... | verify --manifest FILE --artifact-dir DIR");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    process.stderr.write(`desktop release error: ${error instanceof Error ? error.message : "unknown failure"}\n`);
    process.exitCode = 1;
  });
}
