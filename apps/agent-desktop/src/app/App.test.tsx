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
const previousRunId = "55555555-5555-4555-8555-555555555555";
const workflow = "enterprise-engineering-readonly-v1";
const localWriteWorkflow = "enterprise-engineering-localwrite-v1";
const waitId = "33333333-3333-4333-8333-333333333333";

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

function readiness(overrides: Record<string, unknown> = {}) {
  return {
    version: 1,
    overall: "ready",
    dependencies: [
      { dependency: "model", status: "ready", code: "ready", checked_unix_seconds: 1 },
      { dependency: "knowledge", status: "ready", code: "ready", checked_unix_seconds: 1 },
      { dependency: "knowledge:standards", status: "ready", code: "ready", checked_unix_seconds: 1 },
      { dependency: "containment", status: "ready", code: "bounded_probe_passed", checked_unix_seconds: 1 },
    ],
    workflows: [
      { workflow, enabled: true, required: true, status: "ready" },
      { workflow: localWriteWorkflow, enabled: true, required: false, status: "ready" },
    ],
    runtime: {
      principal: { principal_id: "desktop-user", kind: "human", roles: ["user", "approver"] },
      max_active_runs: 8,
      max_run_input_bytes: 16_384,
      max_read_page_items: 64,
      workflow_budgets: [
        { workflow_id: workflow, max_model_calls: 1, max_tool_calls: 0, max_iterations: 0, max_approval_requests: 0, max_graph_steps: 8, max_elapsed_millis: 30_000 },
        { workflow_id: localWriteWorkflow, max_model_calls: 1, max_tool_calls: 1, max_iterations: 0, max_approval_requests: 1, max_graph_steps: 12, max_elapsed_millis: 60_000 },
      ],
    },
    reconciliation: { access: "not_authorized" },
    ...overrides,
  };
}

function installServiceMock(handler?: (command: string, args?: Record<string, unknown>) => unknown) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    const custom = handler?.(command, args);
    if (custom !== undefined) return Promise.resolve(custom);
    if (command === "desktop_build_info") return Promise.resolve({ application: "enterprise-local-agent-desktop", version: "0.1.0", git_revision: "b396a10028cb8bcc6d9420f12e93ec422351dd61", build_profile: "release", service_api_version: 1, service_event_version: 2 });
    if (command === "service_health") return Promise.resolve({ version: 1, lifecycle: "serving" });
    if (command === "service_compatibility") return Promise.resolve({ state: "compatible", service_generation: "77777777-7777-4777-8777-777777777777" });
    if (command === "service_readiness") return Promise.resolve(readiness());
    if (command === "service_version") return Promise.resolve({ application: "enterprise-local-agent", version: "0.1.0", config_schema_version: 2, store_schema_version: 2, event_schema_version: 9, checkpoint_schema_version: 4 });
    if (command === "conversation_create_session") return Promise.resolve({ session_id: sessionId });
    if (command === "conversation_list_sessions") return Promise.resolve({ items: [], next_session_id: null });
    if (command === "conversation_list_runs") return Promise.resolve({ items: [], next_run_id: null });
    if (command === "conversation_start_readonly_run") return Promise.resolve(runView());
    if (command === "conversation_read_events") return Promise.resolve([]);
    if (command === "conversation_run_status") return Promise.resolve(runView());
    if (command === "conversation_cancel_run") return Promise.resolve(null);
    if (command === "approval_list_waiting") return Promise.resolve({ items: [], next_run_id: null, next_wait_id: null });
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

  it("auto-grows the composer and enforces the bounded UTF-8 input before sending", async () => {
    installServiceMock();
    renderApp();
    const composer = await screen.findByRole("textbox", { name: "Task composer" });
    await waitFor(() => expect(composer).toBeEnabled());
    Object.defineProperty(composer, "scrollHeight", { configurable: true, value: 132 });
    fireEvent.change(composer, { target: { value: "Line one\nLine two\nLine three" } });
    expect(composer).toHaveStyle({ height: "132px" });

    fireEvent.change(composer, { target: { value: "x".repeat(8_193) } });
    expect(composer).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("8,193 / 8,192 bytes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send task" })).toBeDisabled();
  });

  it("renders readable GFM tables while keeping model-provided links inert", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") return [event(0, "model", "completed")];
      if (command === "conversation_run_status") return runView({
        disposition: "completed",
        last_sequence: 0,
        outcome: "completed",
        result: {
          kind: "final_answer",
          answer: "| State | Meaning |\n| --- | --- |\n| Ready | Safe |\n\n[External reference](javascript:alert(1))\n\n![Remote image](file:///etc/passwd)\n\n<img src=x onerror=alert(1)>",
          citations: [],
        },
      });
      return undefined;
    });
    renderApp();
    await submit("Render structured answer");
    expect(await screen.findByRole("table")).toBeInTheDocument();
    expect(screen.getByText("External reference")).toBeInTheDocument();
    expect(screen.getByText("[Image omitted: Remote image]")).toBeInTheDocument();
    expect(document.querySelector("a[href]")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
  });

  it.each([
    ["legacy_unsupported", "Service upgrade required"],
    ["incompatible", "Service upgrade required"],
  ] as const)("blocks runtime commands for %s compatibility", async (state, message) => {
    installServiceMock((command) => command === "service_compatibility"
      ? { state, service_generation: null }
      : undefined);
    renderApp();

    expect(await screen.findByRole("heading", { name: message })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task composer" })).toBeDisabled();
    expect(invoke.mock.calls.some(([command]) => command === "conversation_list_sessions")).toBe(false);
    expect(invoke.mock.calls.some(([command]) => command === "conversation_start_readonly_run")).toBe(false);
  });

  it("starts only the fixed LocalWrite workflow when explicitly selected", async () => {
    installServiceMock((command) => {
      if (command === "conversation_start_localwrite_run") return runView({ workflow_id: localWriteWorkflow });
      return undefined;
    });
    renderApp();
    const localWrite = await screen.findByRole("button", { name: "Local write" });
    await waitFor(() => expect(localWrite).toBeEnabled());
    fireEvent.click(localWrite);
    await submit("Create the reviewed report");
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_localwrite_run")).toHaveLength(1);
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_readonly_run")).toHaveLength(0);
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
        if (reads < 2) return runView();
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
    expect(await screen.findByText("Result unavailable")).toBeInTheDocument();
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_readonly_run")).toHaveLength(1);
  });

  it("restores durable conversation history without starting a duplicate run", async () => {
    installServiceMock((command) => {
      if (command === "conversation_list_sessions") return {
        items: [{
          session_id: sessionId,
          title: "Conversation 11111111",
          last_activity_unix_millis: 1_000,
          latest_run: { run_id: runId, disposition: "completed", last_sequence: 0, outcome: "completed", workflow_id: workflow, started_at_unix_millis: 1_000, result_available: true },
        }],
        next_session_id: null,
      };
      if (command === "conversation_list_runs") return {
        items: [
          { run_id: runId, disposition: "completed", last_sequence: 0, outcome: "completed", workflow_id: workflow, started_at_unix_millis: 1_000, result_available: true },
          { run_id: previousRunId, disposition: "failed", last_sequence: 0, outcome: "failed", workflow_id: workflow, started_at_unix_millis: 900, result_available: false },
        ],
        next_run_id: null,
      };
      if (command === "conversation_read_events") return [event(0, "graph", "completed")];
      if (command === "conversation_run_status") return runView({ disposition: "completed", last_sequence: 0, outcome: "completed", result: { kind: "final_answer", answer: "Restored durable answer", citations: [] } });
      return undefined;
    });
    renderApp();
    expect((await screen.findAllByText("Conversation 11111111")).length).toBeGreaterThanOrEqual(1);
    expect(await screen.findByText("Restored durable answer")).toBeInTheDocument();
    expect(invoke.mock.calls.some(([command]) => command.startsWith("conversation_start_"))).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Show inspector" }));
    expect(await screen.findByRole("heading", { name: "Previous runs" })).toBeInTheDocument();
    expect(screen.getByText(previousRunId.slice(0, 8))).toBeInTheDocument();
  });

  it("paginates session summaries and switches conversations lazily", async () => {
    const secondSessionId = "44444444-4444-4444-8444-444444444444";
    installServiceMock((command, args) => {
      if (command === "conversation_list_sessions") {
        return args?.afterSessionId === null
          ? { items: [{ session_id: sessionId, title: "First conversation", last_activity_unix_millis: null, latest_run: null }], next_session_id: secondSessionId }
          : { items: [{ session_id: secondSessionId, title: "Second conversation", last_activity_unix_millis: null, latest_run: null }], next_session_id: null };
      }
      if (command === "conversation_list_runs") return { items: [], next_run_id: null };
      return undefined;
    });
    renderApp();
    expect((await screen.findAllByText("First conversation")).length).toBeGreaterThanOrEqual(1);
    expect((await screen.findAllByText("Second conversation")).length).toBeGreaterThanOrEqual(1);
    fireEvent.click(screen.getByRole("button", { name: /Second conversation/ }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("conversation_list_runs", {
      sessionId: secondSessionId,
      afterRunId: null,
    }));
  });

  it("loads 100 session summaries without eagerly loading run payloads", async () => {
    const summaries = Array.from({ length: 100 }, (_, index) => ({
      session_id: `00000000-0000-4000-8000-${index.toString().padStart(12, "0")}`,
      title: `History ${index.toString().padStart(3, "0")}`,
      last_activity_unix_millis: index,
      latest_run: null,
    }));
    const started = performance.now();
    installServiceMock((command, args) => {
      if (command === "conversation_list_sessions") {
        return args?.afterSessionId === null
          ? { items: summaries.slice(0, 64), next_session_id: summaries[63]?.session_id }
          : { items: summaries.slice(64), next_session_id: null };
      }
      if (command === "conversation_list_runs") return { items: [], next_run_id: null };
      return undefined;
    });

    renderApp();
    expect((await screen.findAllByText("History 000")).length).toBeGreaterThanOrEqual(1);
    expect((await screen.findAllByText("History 099")).length).toBeGreaterThanOrEqual(1);
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_list_sessions")).toHaveLength(2);
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_list_runs")).toHaveLength(1);
    expect(invoke.mock.calls.some(([command]) => command === "conversation_read_events")).toBe(false);
    console.info(`100-session render elapsed_ms=${Math.round(performance.now() - started)}`);
  });

  it("restores a completed run without a volatile answer as Result unavailable", async () => {
    installServiceMock((command) => {
      if (command === "conversation_list_sessions") return {
        items: [{
          session_id: sessionId,
          title: "Restarted conversation",
          last_activity_unix_millis: 1_000,
          latest_run: { run_id: runId, disposition: "completed", last_sequence: 0, outcome: "completed", workflow_id: workflow, started_at_unix_millis: 1_000, result_available: false },
        }],
        next_session_id: null,
      };
      if (command === "conversation_list_runs") return {
        items: [{ run_id: runId, disposition: "completed", last_sequence: 0, outcome: "completed", workflow_id: workflow, started_at_unix_millis: 1_000, result_available: false }],
        next_run_id: null,
      };
      if (command === "conversation_read_events") return [event(0, "graph", "completed")];
      if (command === "conversation_run_status") return runView({ disposition: "completed", last_sequence: 0, outcome: "completed", result: null });
      return undefined;
    });
    renderApp();
    expect(await screen.findByText("Result unavailable")).toBeInTheDocument();
    expect(invoke.mock.calls.some(([command]) => command.startsWith("conversation_start_"))).toBe(false);
    expect(window.localStorage.length).toBe(0);
  });

  it("labels a durable cancelled run as cancelled rather than failed", async () => {
    installServiceMock((command) => {
      if (command === "conversation_list_sessions") return {
        items: [{
          session_id: sessionId,
          title: "Cancelled task",
          last_activity_unix_millis: 1_000,
          latest_run: { run_id: runId, disposition: "failed", last_sequence: 0, outcome: "cancelled", workflow_id: workflow, started_at_unix_millis: 900, result_available: false },
        }],
        next_session_id: null,
      };
      if (command === "conversation_list_runs") return {
        items: [{ run_id: runId, disposition: "failed", last_sequence: 0, outcome: "cancelled", workflow_id: workflow, started_at_unix_millis: 900, result_available: false }],
        next_run_id: null,
      };
      if (command === "conversation_run_status") return runView({ disposition: "failed", last_sequence: 0, outcome: "cancelled" });
      return undefined;
    });
    renderApp();
    expect(await screen.findByText("Cancelled")).toBeInTheDocument();
    expect(screen.queryByText("Failed")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Run needs attention")).not.toBeInTheDocument();
  });

  it("styles a cancelled run distinctly from a failure", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") return [event(0, "graph", "failed")];
      if (command === "conversation_run_status") return runView({ disposition: "failed", last_sequence: 0, outcome: "cancelled" });
      return undefined;
    });
    renderApp();
    await submit("Cancel this task");
    const message = await screen.findByText("This run was cancelled.");
    expect(message.closest("article")).toHaveClass("message-cancelled");
    expect(screen.getByLabelText("Run summary")).toHaveTextContent("Cancelled");
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

  it("shows manual reconciliation as non-resumable without approval controls", async () => {
    installServiceMock((command) => {
      if (command === "conversation_read_events") return [event(0, "graph", "failed")];
      if (command === "conversation_run_status") return runView({ disposition: "manual_reconciliation_required", last_sequence: 0, outcome: "failed" });
      return undefined;
    });
    renderApp();
    await submit("Legacy waiting state");
    expect(await screen.findByText("This run requires operator reconciliation and cannot be resumed from the desktop.")).toBeInTheDocument();
    expect(screen.getByLabelText("Run summary")).toHaveTextContent("Reconciliation required");
    expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Resume/ })).not.toBeInTheDocument();
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

  it("restores durable Waiting, renders the trusted preview, and approves before explicit resume", async () => {
    let state: "waiting" | "approved" = "waiting";
    installServiceMock((command) => {
      if (command === "approval_list_waiting") return { items: [{ session_id: sessionId, run_id: runId, wait_id: waitId, row_version: state === "waiting" ? 0 : 1, state }], next_run_id: null, next_wait_id: null };
      if (command === "approval_get_preview") return { wait_id: waitId, row_version: state === "waiting" ? 0 : 1, operation: "Write workspace file", target: "reports/result.txt", content_bytes: 28 };
      if (command === "approval_submit_decision") { state = "approved"; return null; }
      if (command === "approval_resume_run") return null;
      if (command === "conversation_run_status") return runView({ workflow_id: localWriteWorkflow, disposition: "waiting" });
      return undefined;
    });
    renderApp();

    expect(await screen.findByRole("heading", { name: "LocalWrite approval required" })).toBeInTheDocument();
    expect(screen.getByText("reports/result.txt")).toBeInTheDocument();
    expect(screen.getByText("28 bytes")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
    expect(invoke.mock.calls.some(([command]) => command === "conversation_start_localwrite_run")).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(await screen.findByRole("button", { name: "Resume approved run" })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("approval_submit_decision", {
      sessionId,
      runId,
      waitId,
      expectedRowVersion: 0,
      decision: "approve",
    });
    expect(invoke.mock.calls.some(([command]) => command === "approval_resume_run")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Resume approved run" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("approval_resume_run", { sessionId, runId, waitId }));
  });

  it("records denial without execution and requires an explicit denied-run continuation", async () => {
    let state: "waiting" | "denied" = "waiting";
    installServiceMock((command) => {
      if (command === "approval_list_waiting") return { items: [{ session_id: sessionId, run_id: runId, wait_id: waitId, row_version: state === "waiting" ? 1 : 2, state }], next_run_id: null, next_wait_id: null };
      if (command === "approval_get_preview") return { wait_id: waitId, row_version: 1, operation: "Write workspace file", target: "denied.txt", content_bytes: 6 };
      if (command === "approval_submit_decision") { state = "denied"; return null; }
      if (command === "conversation_run_status") return runView({ workflow_id: localWriteWorkflow, disposition: "waiting" });
      return undefined;
    });
    renderApp();
    const deny = await screen.findByRole("button", { name: "Deny" });
    await waitFor(() => expect(deny).toBeEnabled());
    fireEvent.click(deny);
    expect(await screen.findByRole("button", { name: "Finish denied run" })).toBeInTheDocument();
    expect(invoke.mock.calls.some(([command]) => command === "approval_resume_run")).toBe(false);
    expect(invoke.mock.calls.some(([command]) => command.includes("tool"))).toBe(false);
  });

  it("aborts Waiting through the named command and removes the approval surface", async () => {
    let aborted = false;
    installServiceMock((command) => {
      if (command === "approval_list_waiting") return { items: aborted ? [] : [{ session_id: sessionId, run_id: runId, wait_id: waitId, row_version: 1, state: "waiting" }], next_run_id: null, next_wait_id: null };
      if (command === "approval_get_preview") return { wait_id: waitId, row_version: 1, operation: "Write workspace file", target: "abort.txt", content_bytes: 4 };
      if (command === "approval_abort_waiting") { aborted = true; return null; }
      if (command === "conversation_run_status") return runView({ workflow_id: localWriteWorkflow, disposition: "waiting" });
      return undefined;
    });
    renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "Abort run" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("approval_abort_waiting", { sessionId, runId, waitId, expectedRowVersion: 1 }));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "LocalWrite approval required" })).not.toBeInTheDocument());
  });

  it.each([
    ["unauthorized", "separate authorized approver"],
    ["state_conflict", "changed on the service"],
  ])("sanitizes approval %s failures without exposing action data", async (code, message) => {
    installServiceMock((command) => {
      if (command === "approval_list_waiting") return { items: [{ session_id: sessionId, run_id: runId, wait_id: waitId, row_version: 1, state: "waiting" }], next_run_id: null, next_wait_id: null };
      if (command === "approval_get_preview") return { wait_id: waitId, row_version: 1, operation: "Write workspace file", target: "safe.txt", content_bytes: 2 };
      if (command === "approval_submit_decision") return Promise.reject({ code, capsule: "secret", action: "payload" });
      if (command === "conversation_run_status") return runView({ workflow_id: localWriteWorkflow, disposition: "waiting" });
      return undefined;
    });
    renderApp();
    const approve = await screen.findByRole("button", { name: "Approve" });
    await waitFor(() => expect(approve).toBeEnabled());
    fireEvent.click(approve);
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(document.body.textContent).not.toContain("capsule");
    expect(document.body.textContent).not.toContain("payload");
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
    expect(screen.getByRole("textbox", { name: "Search commands" })).toHaveAttribute("aria-activedescendant", "command-new-task");
  });

  it("shows a sanitized unavailable state", async () => {
    invoke.mockRejectedValue(new Error("credential=secret"));
    renderApp();
    expect(await screen.findByRole("heading", { name: "Local service unavailable" })).toBeInTheDocument();
    expect(screen.queryByText(/credential=secret/)).not.toBeInTheDocument();
  });

  it("shows unavailable after a previously healthy service disconnects", async () => {
    let healthAvailable = true;
    installServiceMock((command) => {
      if (command === "service_health" && !healthAvailable) return Promise.reject(new Error("offline"));
      return undefined;
    });
    renderApp();
    expect(await screen.findByText("Local service ready")).toBeInTheDocument();

    healthAvailable = false;
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.change(screen.getByRole("textbox", { name: "Search commands" }), { target: { value: "refresh" } });
    fireEvent.click(screen.getByRole("option", { name: /Refresh service status/ }));

    expect(await screen.findByRole("heading", { name: "Local service unavailable" })).toBeInTheDocument();
  });

  it("refreshes compatibility and durable state after wake or service generation change", async () => {
    let generation = "77777777-7777-4777-8777-777777777777";
    let historyReads = 0;
    installServiceMock((command) => {
      if (command === "service_compatibility") {
        return { state: "compatible", service_generation: generation };
      }
      if (command === "conversation_list_sessions") {
        historyReads += 1;
        return { items: [], next_session_id: null };
      }
      return undefined;
    });
    renderApp();
    await waitFor(() => expect(historyReads).toBeGreaterThanOrEqual(1));

    generation = "88888888-8888-4888-8888-888888888888";
    fireEvent.focus(window);
    await waitFor(() => expect(historyReads).toBeGreaterThanOrEqual(2));
    expect(invoke.mock.calls.filter(([command]) => command === "service_compatibility").length).toBeGreaterThanOrEqual(2);
    expect(invoke.mock.calls.filter(([command]) => command === "conversation_start_readonly_run")).toHaveLength(0);
  });

  it("keeps the task workspace visible in a compact window", async () => {
    window.innerWidth = 800;
    installServiceMock();
    renderApp();
    await screen.findByText("Local service ready");
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task composer" })).toBeInTheDocument();
  });

  it("lazy-loads Settings from the app bar and changes the local theme", async () => {
    installServiceMock();
    renderApp();
    await screen.findByText("Local service ready");
    expect(screen.queryByRole("heading", { name: "Settings" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close settings" })).toHaveFocus();
    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(document.documentElement).toHaveClass("dark");
    expect(window.localStorage.getItem("ela-desktop-theme")).toBe("dark");
    fireEvent.click(screen.getByRole("button", { name: "Close settings" }));
    expect(await screen.findByRole("textbox", { name: "Task composer" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Open settings" })).toHaveFocus());
  });

  it("opens Settings from the searchable command palette", async () => {
    installServiceMock();
    renderApp();
    await screen.findByText("Local service ready");
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.click(await screen.findByRole("button", { name: /Open settings/ }));
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
  });

  it("renders safe workflow, identity, readiness, and unavailable LocalWrite status", async () => {
    installServiceMock((command) => {
      if (command !== "service_readiness") return undefined;
      return readiness({
        overall: "degraded",
        dependencies: [
          { dependency: "model", status: "degraded", code: "probe_failed", checked_unix_seconds: 1 },
          { dependency: "knowledge", status: "ready", code: "ready", checked_unix_seconds: 1 },
          { dependency: "containment", status: "unavailable", code: "unavailable", checked_unix_seconds: 1 },
        ],
        workflows: [
          { workflow, enabled: true, required: true, status: "ready" },
          { workflow: localWriteWorkflow, enabled: false, required: false, status: "unavailable" },
        ],
      });
    });
    renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "Open settings" }));

    expect(await screen.findByText("desktop-user")).toBeInTheDocument();
    expect(screen.getByText("user · approver")).toBeInTheDocument();
    expect(screen.getByText("LocalWrite").parentElement?.parentElement).toHaveTextContent("Unavailable");
    expect(screen.getByText("Model provider").parentElement?.parentElement).toHaveTextContent("Degraded");
    expect(screen.getByText("Containment").parentElement?.parentElement).toHaveTextContent("Unavailable");
    expect(screen.getByText("Desktop application").parentElement?.parentElement).toHaveTextContent("0.1.0");
    expect(screen.getByText("Service build").parentElement?.parentElement).toHaveTextContent("enterprise-local-agent 0.1.0");
  });

  it("shows reconciliation counts only for an operator-authorized projection", async () => {
    installServiceMock((command) => {
      if (command !== "service_readiness") return undefined;
      const base = readiness();
      return readiness({
        runtime: {
          ...base.runtime,
          principal: { principal_id: "ops-user", kind: "human", roles: ["operator"] },
        },
        reconciliation: { access: "authorized", count: 1, truncated: false },
      });
    });
    renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "Open settings" }));
    fireEvent.click(await screen.findByText("Advanced operations"));
    expect(screen.getByText("1 run requires attention")).toBeInTheDocument();
    expect(screen.getByText("ops-user")).toBeInTheDocument();
  });

  it("does not render security configuration or credential-bearing metadata", async () => {
    installServiceMock();
    renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "Open settings" }));
    fireEvent.click(await screen.findByText("Advanced operations"));
    const visible = document.body.textContent ?? "";
    for (const forbidden of ["deployment_fingerprint", "git_identity", "credential", "token", "seal key", "/usr/bin", "capsule"]) {
      expect(visible).not.toContain(forbidden);
    }
    expect(screen.getByText("Operator access required")).toBeInTheDocument();
  });
});
