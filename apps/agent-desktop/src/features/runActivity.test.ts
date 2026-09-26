import { describe, expect, it } from "vitest";

import type { RunView, ServiceEvent } from "@/bridge/contracts";
import { mergeEventPage, runDetails } from "@/features/runActivity";
import { pollingInterval, validatedCursor } from "@/queries/conversation";

const runId = "22222222-2222-4222-8222-222222222222";
const workflow = "enterprise-engineering-readonly-v1" as const;

function event(sequence: number, overrides: Partial<ServiceEvent> = {}): ServiceEvent {
  return {
    version: 2,
    sequence,
    run_id: runId,
    category: "knowledge",
    phase: "started",
    correlation_id: `correlation-${sequence}`,
    workflow_id: workflow,
    knowledge_backends: ["standards"],
    graph_node_id: null,
    budget_usage: null,
    budget_limit: null,
    ...overrides,
  };
}

function status(overrides: Partial<RunView> = {}): RunView {
  return {
    session_id: "11111111-1111-4111-8111-111111111111",
    run_id: runId,
    disposition: "running",
    last_sequence: null,
    outcome: null,
    workflow_id: workflow,
    result: null,
    duration_millis: null,
    ...overrides,
  };
}

describe("run activity metadata", () => {
  it("uses bounded adaptive polling and stops after terminal state", () => {
    expect(pollingInterval("running", true)).toBe(400);
    expect(pollingInterval("waiting", true)).toBe(2_000);
    expect(pollingInterval("running", false)).toBe(3_000);
    expect(pollingInterval("completed", true)).toBe(false);
    expect(pollingInterval("manual_reconciliation_required", true)).toBe(false);
  });

  it("invalidates a cursor ahead of the authoritative service sequence", () => {
    expect(validatedCursor(9, 4)).toEqual({ cursor: null, stale: true });
    expect(validatedCursor(0, null)).toEqual({ cursor: null, stale: true });
    expect(validatedCursor(4, 4)).toEqual({ cursor: 4, stale: false });
    expect(validatedCursor(null, 4)).toEqual({ cursor: null, stale: false });
  });

  it("accepts sequence zero, ignores duplicates, and advances contiguously", () => {
    const first = mergeEventPage([], null, [event(0), event(0)], runId);
    expect(first).toMatchObject({ valid: true, cursor: 0 });
    expect(first.events).toHaveLength(1);

    const second = mergeEventPage(first.events, first.cursor, [event(0), event(1)], runId);
    expect(second).toMatchObject({ valid: true, cursor: 1 });
    expect(second.events.map(({ sequence }) => sequence)).toEqual([0, 1]);
  });

  it("rejects gaps, out-of-order pages, and events for another run", () => {
    const existing = [event(0)];
    expect(mergeEventPage(existing, 0, [event(2)], runId).valid).toBe(false);
    expect(mergeEventPage(existing, 0, [event(2), event(1)], runId).valid).toBe(false);
    expect(mergeEventPage(existing, 0, [event(1, { run_id: "33333333-3333-4333-8333-333333333333" })], runId).valid).toBe(false);
  });

  it("derives only safe run metadata and keeps terminal state authoritative", () => {
    const events = [
      event(0),
      event(1, { category: "model", budget_usage: 1, budget_limit: 3 }),
      event(2, { category: "graph", correlation_id: null, graph_node_id: "verify-answer", budget_usage: 3, budget_limit: 8 }),
    ];
    const details = runDetails(
      status({
        disposition: "completed",
        outcome: "completed",
        duration_millis: 12_400,
        result: { kind: "final_answer", answer: "Safe answer", citations: [] },
      }),
      events,
      1_000,
      99_000,
      "Thinking…",
    );

    expect(details.currentPhase).toBe("Complete");
    expect(details.elapsedMillis).toBe(12_400);
    expect(details.modelCount).toBe(1);
    expect(details.graphBudget).toEqual({ usage: 3, limit: 8 });
    expect(details.evidenceCount).toBeNull();
    expect(details.timeline.at(-1)?.state).toBe("completed");
  });
});
