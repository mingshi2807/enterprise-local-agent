import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { createManifest, repositoryRoot, validateReleaseConfig, verifyManifest } from "./desktop-release.mjs";

const revision = "0123456789abcdef0123456789abcdef01234567";

test("release configuration keeps versions, bundles, capabilities, and signing policy aligned", async () => {
  const result = await validateReleaseConfig();
  assert.match(result.version, /^\d+\.\d+\.\d+/);
  assert.equal(result.identifier, "com.enterprise-local-agent.desktop");
});

test("manifest is deterministic and verifies artifact integrity", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "ela-release-"));
  const artifact = path.join(temporary, "agent-desktop_0.1.0_amd64.deb");
  const first = path.join(temporary, "first.json");
  const second = path.join(temporary, "second.json");
  await writeFile(artifact, "bounded desktop artifact\n");

  const options = {
    artifactPaths: [artifact],
    root: repositoryRoot,
    sourceDateEpoch: "1700000000",
    revision,
    target: "linux-x86_64",
    allowDirty: true,
  };
  await createManifest({ ...options, output: first });
  await createManifest({ ...options, output: second });
  assert.equal(await readFile(first, "utf8"), await readFile(second, "utf8"));
  const verified = await verifyManifest({ manifestPath: first, artifactDirectory: temporary, root: repositoryRoot, allowDirty: true });
  assert.equal(verified.artifacts[0].filename, path.basename(artifact));

  await writeFile(artifact, "tampered\n");
  await assert.rejects(
    verifyManifest({ manifestPath: first, artifactDirectory: temporary, root: repositoryRoot, allowDirty: true }),
    /artifact verification failed/,
  );
});

test("directory artifacts use stable sorted tree hashing", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "ela-release-app-"));
  const app = path.join(temporary, "Enterprise Local Agent.app");
  await mkdir(path.join(app, "Contents/MacOS"), { recursive: true });
  await writeFile(path.join(app, "Contents/MacOS/agent-desktop"), "binary");
  await writeFile(path.join(app, "Contents/Info.plist"), "plist");
  const first = path.join(temporary, "app-first.json");
  const second = path.join(temporary, "app-second.json");
  const options = {
    artifactPaths: [app],
    root: repositoryRoot,
    sourceDateEpoch: "1700000000",
    revision,
    target: "macos-aarch64",
    allowDirty: true,
  };
  await createManifest({ ...options, output: first });
  await createManifest({ ...options, output: second });
  assert.equal(await readFile(first, "utf8"), await readFile(second, "utf8"));
});
