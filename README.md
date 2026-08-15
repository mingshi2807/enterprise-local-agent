# Enterprise Local Agent

Local-first Rust agent runtime with an enterprise execution harness, a typed
deterministic outer loop, and replaceable model/provider adapters.

## Deterministic demonstration

The default CLI path is network-free. It uses `RigModelAdapter<FakeRigModel>`
and the deterministic ReadOnly fake tool:

```bash
cargo run -p agent-cli
```

It prints only run identifiers, provider label, loop progression, final status,
budget usage, and audit-degraded status.

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

M4 intentionally does not provide readiness calls, retries, proxy settings,
custom certificate authorities, mTLS, streaming, structured output, model
fallback, or model-driven tool execution.
