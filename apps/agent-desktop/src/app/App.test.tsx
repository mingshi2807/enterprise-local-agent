import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { ThemeProvider } from "@/app/ThemeProvider";
import { serviceEventSchema } from "@/bridge/contracts";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const sessionId = "11111111-1111-4111-8111-111111111111";
const runId = "22222222-2222-4222-8222-222222222222";
const workflow = "enterprise-engineering-readonly-v1";

function renderApp() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  return render(<ThemeProvider><QueryClientProvider client={queryClient}><App /></QueryClientProvider></ThemeProvider>);
}

function runView(overrides: Record<string, unknown> = {}) {
  return { session_id: sessionId, run_id: runId, disposition: "running", last_sequence: null, outcome: null, workflow_id: workflow, result: null, duration_millis: null, ...overrides };
}

function event(sequence: number, category: "knowledge" | "model" | "graph", phase: "started" | "completed" | "failed", graphNodeId: string | null = null) {
  return { version: 2, sequence, run_id: runId, category, phase, correlation_id: `${category}-${sequence}`, workflow_id: workflow, knowledge_backends: category === "knowledge" ? ["standards"] : [], graph_node_id: graphNodeId, budget_usage: null, budget_limit: null };
}

function installServiceMock(handler?: (command: string, args?: Record<string, unknown>) => unknown) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    const custom = handler?.(command, args);
    if (custom !== undefined) return Promise.resolve(custom);
    if (command === "service_health") return Promise.resolve({ version: 1, lifecycle: "serving" });
    if (command === "service_readiness") return Promise.resolve({ version: 1, overall: "ready", dependencies: [{ dependency: "knowledge", status: "ready", code: "ready", checked_unix_seconds: 1 }], workflows: [{ workflow, enabled: true, required: true, status: "ready" }] });
    if (command === "service_version") return Promise.resolve({ application: "enterprise-local-agent", version: "0.1.0", git_identity: null, deployment_fingerprint: "test", config_schema_version: 2, store_schema_version: 2, event_schema_version: 9, checkpoint_schema_version: 4 });
    if (command === "conversation_create_session") return Promise.resolve({ session_id: sessionId });
    if (command === "conversation_start_readonly_run") return Promise.resolve(runView());
    if (command === "conversation_read_events") return Promise.resolve([]);
    if (command === "conversation_run_status") return Promise.resolve(runView());
    if (command === "conversation_cancel_run") return Promise.resolve(null);
    return Promise.reject(new Error("unexpected_command"));
  });
}

async function submit(prompt = "Explain the charging requirement") {
  const composer = await screen.findByRole("textbox", { name: "Task composer" });
  await waitFor(() => expect(composer).toBeEnabled());
  fireEvent.change(composer, { target: { value: prompt } });
  fireEvent.keyDown(composer, { key: "Enter", ctrlKey: true });
  await waitFor(() => expect(screen.getAllByText(prompt).length).toBeGreaterThanOrEqual(1));
}

describe("App conversation", () => {
  afterEach(cleanup);
  beforeEach(() => { invoke.mockReset(); window.localStorage.clear(); window.innerWidth = 1280; });

  it("starts the fixed ReadOnly workflow and renders the authoritative answer and citations", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") return [event(0, "knowledge", "started"), event(1, "model", "completed")];
      if (command === "conversation_run_status") return runView({ disposition: "completed", last_sequence: 1, outcome: "completed", result: { kind: "final_answer", answer: "Use **bounded** charging control.\n\n```rust\nlet safe = true;\n```", citations: [{ evidence_id: "ev-1", backend: "standards", source_id: "iso", reference_id: "8.4", provenance: "ISO reference" }] }, duration_millis: 42 });
      return undefined;
    });
    renderApp();
    await submit();

    expect(await screen.findByText("bounded")).toBeInTheDocument();
    expect(screen.getByText(/standards · 8.4/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy answer" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeInTheDocument();
    expect(screen.getByLabelText("Run summary")).toHaveTextContent(/Completed.*42ms.*1 source/);
    const start = invoke.mock.calls.find(([command]) => command === "conversation_start_readonly_run");
    expect(start?.[1]).toMatchObject({ sessionId, input: "Explain the charging requirement" });
    expect(start?.[1]?.startRequestId).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("allows only one active send and cancels through the named command", async () => {
    installServiceMock();
    renderApp();
    await submit("Long running task");
    expect(await screen.findByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task composer" })).toBeDisabled();
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_readonly_run")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("conversation_cancel_run", { sessionId, runId }));
  });

  it("shows compact activity and exposes safe metadata only on demand", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") {
        return [
          event(0, "knowledge", "started"),
          event(1, "knowledge", "completed"),
          { ...event(2, "model", "started"), budget_usage: 1, budget_limit: 3 },
          { ...event(3, "graph", "started", "verify-answer"), correlation_id: null, budget_usage: 3, budget_limit: 8 },
        ];
      }
      if (command === "conversation_run_status") return runView({ last_sequence: 3 });
      return undefined;
    });
    renderApp();
    await submit("Inspect this run");

    expect((await screen.findAllByText("Verifying…")).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.queryByText("KnowledgeRetrievalStarted")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Show inspector" }));
    expect(await screen.findByRole("heading", { name: "Run details" })).toBeInTheDocument();
    expect(screen.getByText("standards")).toBeInTheDocument();
    expect(screen.getByText("3 / 8")).toBeInTheDocument();
    expect(screen.getByText("Not exposed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy Run" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy Retrieval 1" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy Model 1" })).toBeInTheDocument();
  });

  it.each([
    ["knowledge", "Enterprise knowledge retrieval is unavailable or timed out."],
    ["model", "The configured model is unavailable or timed out."],
  ] as const)("shows a sanitized %s failure and retries as a new run", async (category, message) => {
    let starts = 0;
    installServiceMock((command) => {
      if (command === "conversation_start_readonly_run") { starts += 1; return runView(); }
      if (command === "conversation_read_events") return [event(0, category, "failed")];
      if (command === "conversation_run_status") return runView({ disposition: "failed", last_sequence: 0, outcome: "failed" });
      return undefined;
    });
    renderApp();
    await submit("Fail safely");
    expect(await screen.findByText(message)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry as new run" }));
    await waitFor(() => expect(starts).toBe(2));
  });

  it("catches up from the last ServiceEventV2 cursor after a transient failure", async () => {
    const cursors: unknown[] = [];
    let reads = 0;
    installServiceMock((command, args) => {
      if (command === "conversation_read_events") {
        reads += 1;
        cursors.push(args?.afterSequence);
        if (reads === 1) return Promise.reject(new Error("service_unavailable"));
        if (reads === 2) return [event(0, "knowledge", "started")];
        return [event(1, "graph", "completed", "verify-answer")];
      }
      if (command === "conversation_run_status") {
        if (reads < 3) return runView();
        return runView({ disposition: "completed", last_sequence: 1, outcome: "completed", result: { kind: "final_answer", answer: "Recovered answer", citations: [] } });
      }
      return undefined;
    });
    renderApp();
    await submit("Reconnect test");
    expect(await screen.findByText("Recovered answer", {}, { timeout: 3_000 })).toBeInTheDocument();
    expect(cursors).toContain(null);
    expect(cursors).toContain(0);
  });

  it("reports volatile result loss without replaying the model call", async () => {
    installServiceMock((command) => {
      if (command === "conversation_run_status") return runView({ disposition: "completed", outcome: "completed", last_sequence: 0 });
      if (command === "conversation_read_events") return [event(0, "graph", "completed")];
      return undefined;
    });
    renderApp();
    await submit("Restarted run");
    expect(await screen.findByText("This run completed, but its answer is no longer available after restart.")).toBeInTheDocument();
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_readonly_run")).toHaveLength(1);
  });

  it("reports a malformed terminal model result without exposing raw output", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") return [event(0, "model", "completed")];
      if (command === "conversation_run_status") return runView({ disposition: "failed", outcome: "failed", last_sequence: 0 });
      return undefined;
    });
    renderApp();
    await submit("Strict output test");
    expect(await screen.findByText("The model returned a result that did not match the required answer format.")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("raw_model_output");
  });

  it("shows a sanitized start failure and keeps the prompt editable", async () => {
    installServiceMock((command) => {
      if (command === "conversation_create_session") return Promise.reject(new Error("credential=secret"));
      return undefined;
    });
    renderApp();
    const composer = await screen.findByRole("textbox", { name: "Task composer" });
    await waitFor(() => expect(composer).toBeEnabled());
    fireEvent.change(composer, { target: { value: "Keep this prompt" } });
    fireEvent.keyDown(composer, { key: "Enter", ctrlKey: true });
    expect(await screen.findByRole("alert")).toHaveTextContent("could not start this task");
    expect(screen.getByRole("textbox", { name: "Task composer" })).toHaveValue("Keep this prompt");
    expect(document.body.textContent).not.toContain("credential=secret");
  });

  it("keeps internal payload fields outside ServiceEventV2", () => {
    expect(serviceEventSchema.safeParse({ ...event(1, "model", "started"), prompt: "secret" }).success).toBe(false);
    expect(serviceEventSchema.safeParse({ ...event(1, "model", "started"), raw_model_output: "secret" }).success).toBe(false);
  });
});

describe("App shell states", () => {
  afterEach(cleanup);
  beforeEach(() => { invoke.mockReset(); window.innerWidth = 1280; });

  it("shows readiness and supports shell shortcuts", async () => {
    installServiceMock();
    renderApp();
    await screen.findByText("Local service ready");
    fireEvent.keyDown(window, { key: "i", ctrlKey: true, shiftKey: true });
    expect(await screen.findByText("No run selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hide inspector" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "b", ctrlKey: true });
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("shows a sanitized unavailable state", async () => {
    invoke.mockRejectedValue(new Error("credential=secret"));
    renderApp();
    expect(await screen.findByRole("heading", { name: "Local service unavailable" })).toBeInTheDocument();
    expect(screen.queryByText(/credential=secret/)).not.toBeInTheDocument();
  });

  it("keeps the task workspace visible in a compact window", async () => {
    window.innerWidth = 800;
    installServiceMock();
    renderApp();
    await screen.findByText("Local service ready");
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task composer" })).toBeInTheDocument();
  });
});
