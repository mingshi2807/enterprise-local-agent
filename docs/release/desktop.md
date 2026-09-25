# Desktop Release Engineering

M16.9 packages only the Tauri desktop client. Production deployments install
and configure `agent-service-daemon` separately. The desktop package does not
embed, install, launch, or update the service, Bubblewrap, the LocalWrite
worker, a model, or enterprise knowledge backends.

On Unix, the desktop first honors the trusted process-level
`ELA_DESKTOP_SERVICE_SOCKET` override. Otherwise it uses
`$XDG_RUNTIME_DIR/enterprise-local-agent.sock` when available, then the fixed
deployment socket `/run/enterprise-local-agent/agent.sock` on Linux or
`/var/run/enterprise-local-agent/agent.sock` on macOS. A missing socket is a
normal unavailable-service UI state, not a desktop startup failure. Loopback
transport remains explicit and requires the existing Rust-side URL, bearer,
and Origin configuration.

## Release Contract

- Authoritative version: `[workspace.package].version` in the repository root
  `Cargo.toml`.
- Checked mirrors: desktop `package.json` and `package-lock.json`.
- Tauri version: inherited from the desktop Cargo package; `tauri.conf.json`
  must not contain a version.
- Bundle identifier: `com.enterprise-local-agent.desktop`.
- Service compatibility: HTTP API v1 and `ServiceEventV2`.
- Updates: manual installation only. Tauri updater artifacts and automatic
  update checks remain disabled.
- Release manifests require a clean worktree and `SOURCE_DATE_EPOCH`. The
  development-only `ELA_RELEASE_ALLOW_DIRTY=1` override produces a manifest
  marked `source_dirty: true`, which is not releasable.

Run configuration and manifest tests before packaging:

```bash
cd apps/agent-desktop
npm ci
npm run release:validate
npm run release:test
npm run typecheck
npm run lint
npm test
npm run build
```

Release builds should also pass repository Rust formatting, strict Clippy, and
workspace tests before artifacts are signed or published.

## Linux x86_64

The initial Linux artifact is a Debian package. AppImage is deferred because it
does not improve the initial managed Debian deployment enough to justify a
second WebKitGTK/runtime packaging path.

Build on Ubuntu 22.04 or Debian 12, the oldest supported build baseline, with
Tauri's WebKitGTK 4.1 prerequisites installed:

```bash
export ELA_GIT_REVISION="$(git rev-parse HEAD)"
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
cd apps/agent-desktop
npm run tauri -- build --bundles deb --ci --no-sign
cd ../..
```

The Tauri bundler generates the desktop entry, icons, package control metadata,
and WebKitGTK/GTK dependencies. Verify the package layout and dynamic runtime
closure without installing it:

```bash
apps/agent-desktop/scripts/verify-linux-deb.sh \
  "target/release/bundle/deb/Enterprise Local Agent Desktop_0.1.0_amd64.deb"
```

Install/uninstall testing must run in a disposable Debian-compatible VM or
container, never on a release operator's workstation:

```bash
dpkg -i "/release/Enterprise Local Agent Desktop_0.1.0_amd64.deb"
test -x /usr/bin/agent-desktop
dpkg -r enterprise-local-agent-desktop
test ! -e /usr/bin/agent-desktop
```

## macOS Apple Silicon

The initial macOS target is `aarch64-apple-darwin`, with macOS 12.0 as the
minimum system version. The bundle uses the hardened runtime and an empty
entitlements file; no sandbox or hardened-runtime exception is granted.

Unsigned development packaging requires no Apple credentials:

```bash
export ELA_GIT_REVISION="$(git rev-parse HEAD)"
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
cd apps/agent-desktop
npm run tauri -- build --target aarch64-apple-darwin \
  --bundles app,dmg --ci --no-sign
scripts/verify-macos.sh unsigned \
  "../../target/aarch64-apple-darwin/release/bundle/macos/Enterprise Local Agent.app" \
  "../../target/aarch64-apple-darwin/release/bundle/dmg/Enterprise Local Agent_0.1.0_aarch64.dmg"
```

Production signing responsibility belongs to the release operator. A Developer
ID Application certificate is imported into an ephemeral CI keychain; Apple
credentials and certificate material remain in the CI secret store and are
deleted with the runner. The repository stores no signing identity, password,
private key, profile, or notarization credential.

The signed flow is:

1. Build the `.app` and DMG with `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
   `APPLE_PASSWORD`, and `APPLE_TEAM_ID` supplied by the release environment.
2. Let Tauri codesign with hardened runtime and submit for notarization.
3. Wait for notarization and staple the ticket.
4. Run `verify-macos.sh signed ...`, which verifies codesign, Gatekeeper,
   stapling, DMG integrity, architecture, identifier, and minimum OS metadata.

Do not use `--skip-stapling` for a published artifact. Normal CI deliberately
uses `--no-sign`; it proves packaging without requiring Apple credentials.

## Manifest and Checksums

Generate one manifest per target after packaging. Artifact paths are sorted,
and file or `.app` tree SHA-256 values are deterministic for identical inputs:

```bash
node apps/agent-desktop/scripts/desktop-release.mjs manifest \
  --target linux-x86_64 \
  --output target/release/enterprise-local-agent-desktop-linux-x86_64.json \
  "target/release/bundle/deb/Enterprise Local Agent Desktop_0.1.0_amd64.deb"

node apps/agent-desktop/scripts/desktop-release.mjs verify \
  --manifest target/release/enterprise-local-agent-desktop-linux-x86_64.json \
  --artifact-dir target/release/bundle/deb
```

The manifest records application version, target, bundle identifier, exact git
revision, release profile, deterministic UTC build timestamp, source cleanliness,
separate-service assumption, update policy, service compatibility, artifact
filenames, sizes, and SHA-256 values. It records no username, hostname,
environment dump, endpoint, credential, path, or signing identity.

## Publication and Updates

Publish only manifests with `source_dirty: false` after package verification.
Distribute the manifest beside the artifacts and verify its SHA-256 values
before installation. Initial release candidates use explicit manual download
and installation. Automatic updates remain out of scope until release signing,
rollback behavior, service compatibility policy, and update-key rotation have
separate review and operational evidence.
