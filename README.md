# Enterprise Local Agent

Local-first Rust agent runtime with an enterprise execution harness, a typed
deterministic outer loop, replaceable model/provider adapters, and governed
model-proposed actions.

## M5 action planning

The Plan phase treats model output as untrusted data. It accepts one strict
provider-neutral JSON envelope, assigns a runtime `ActionProposalId`, validates
the exact tool name and arguments against the registered Draft 2020-12 schema,
and produces a single-use `ValidatedAction`. The Act phase then binds a fresh
`ToolCallId` and delegates policy, budget, audit, and execution to
`ExecutionHarness`.

A proposal is not a `ToolCall` and does not authorize execution. Rig/OpenAI
tool execution, native provider tool calls, retries, and heuristic JSON or
markdown extraction are not enabled.

## M6 approval and containment boundary

M6 keeps capability policy, explicit approval, and technical containment as
independent authorities. ReadOnly actions remain executable without approval.
LocalWrite actions require `M6ApprovalPolicy::RequiresApproval`, a matching
approval decision for the exact validated action, required security audit
records, and a separately registered `ContainedToolPort`. ExternalWrite and
Privileged actions remain non-executable even if a custom policy is faulty.

`ContainedToolPort` is a trusted adapter contract; implementing the Rust trait
alone does not prove operating-system isolation. M6.1 adds the Linux-only
`LinuxWorkspaceWriteTool` for one operation: `workspace_write_file`. It is
usable only after its production capability probe verifies a non-setuid
bubblewrap with FD binding, the self-contained worker, namespaces, no external
network, descriptor/environment isolation, and mandatory `openat2()` flags.
Unsupported hosts fail closed before tool registration.

The worker receives only a final basename and at most 4 KiB of approved UTF-8
content. The trusted parent resolves the target parent beneath the configured
workspace FD and exposes only that directory as writable. Landlock is reported
and used as optional defense-in-depth; it is not part of the mandatory M6.1
availability decision.

The first certified target requires an immutable, statically linked Linux ELF
worker (executable with no write bits, for example mode `0555`). Run the
fail-closed host certification explicitly after installing bubblewrap 0.11.2+
and that worker artifact:

```bash
ELA_M6_1_BWRAP=/usr/bin/bwrap \
ELA_M6_1_WORKER=/absolute/path/enterprise-local-write-worker \
CARGO_HOME=/tmp/enterprise-local-agent-cargo \
cargo test -p agent-containment-linux --all-features \
  --test linux_certification -- --ignored --exact production_linux_security_certification
```

The probe tests required behavior and does not trust the version string alone.
Invoking certification on an unsupported host is a test failure, not a skip.

## Deterministic demonstration

The default CLI path is network-free. It uses `RigModelAdapter<FakeRigModel>`
to return a strict action envelope, then validates and executes the
deterministic ReadOnly fake tool through the harness:

```bash
cargo run -p agent-cli
```

It prints only run identifiers, provider label, loop progression, final status,
budget usage, and audit-degraded status.

An explicit governance-only demonstration is also available:

```bash
cargo run -p agent-cli -- --demo-local-write-fake-containment
```

That mode uses scripted fake approval and a test fake contained executor. It
does not write files, launch processes, use a network, or provide OS isolation;
the CLI labels both fakes prominently.

## OpenAI-compatible live mode

Live provider access is explicit and is never selected by default:

```bash
ELA_OPENAI_COMPAT_BASE_URL=http://127.0.0.1:8000/v1 \
ELA_OPENAI_COMPAT_MODEL=your-deployed-model-identifier \
ELA_OPENAI_COMPAT_API_KEY=your-bearer-credential \
ELA_OPENAI_COMPAT_LABEL=local-qwen \
cargo run -p agent-cli -- --live-openai-compatible
```

The model identifier is opaque configuration. It may be a concrete deployment,
an enterprise-gateway alias, or a virtual-model identifier.

HTTP is accepted only for `localhost` or an IPv4/IPv6 loopback address. All
other endpoints require HTTPS. The base URL must be an API root such as `/v1`,
not `/v1/chat/completions`. Bearer authentication is mandatory in M4.

The CLI does not print prompts, model responses, credentials, full endpoint
URLs, or model identifiers. Production compositions must not enable TRACE-level
Rig/provider logging without first reviewing the exact pinned Rig version for
content leakage.

The live mode uses the same M5 `ActionProgram`; only model composition changes.
If a live model returns prose, code fences, malformed JSON, or an invalid
action, preparation fails closed without extraction heuristics.

M4-M6.1 intentionally do not provide readiness calls, retries, proxy settings,
custom certificate authorities, mTLS, streaming, structured output, model
fallback, native provider tool calls, provider-driven tool execution,
enterprise identity, or durable approval. M6.1 containment is Linux-only and
limited to `workspace_write_file`; it is not an arbitrary tool sandbox.
