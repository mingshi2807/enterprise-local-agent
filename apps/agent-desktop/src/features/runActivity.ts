import type { RunView, ServiceEvent } from "@/bridge/contracts";

export type ActivityLabel =
  | "Starting…"
  | "Searching knowledge…"
  | "Thinking…"
  | "Approval required"
  | "Waiting for decision…"
  | "Resuming…"
  | "Verifying…"
  | "Finishing…"
  | "Reconnecting…"
  | "Stopping…";

export interface EventMergeResult {
  events: ServiceEvent[];
  cursor: number | null;
  valid: boolean;
}

export interface TimelineItem {
  id: "retrieve" | "model" | "verify" | "complete";
  label: string;
  state: "pending" | "active" | "completed" | "failed" | "cancelled";
}

export interface RunDetails {
  runId: string;
  workflow: string;
  status: string;
  currentPhase: string;
  elapsedMillis: number;
  modelCount: number;
  modelBudget: { usage: number; limit: number } | null;
  graphBudget: { usage: number; limit: number } | null;
  retrievalBackends: string[];
  evidenceCount: null;
  citationCount: number;
  retrievalIds: string[];
  modelIds: string[];
  terminalStatus: string | null;
  timeline: TimelineItem[];
}

export function mergeEventPage(
  existing: ServiceEvent[],
  cursor: number | null,
  page: ServiceEvent[],
  runId: string,
): EventMergeResult {
  const merged = [...existing];
  const known = new Set(existing.map(({ sequence }) => sequence));
  let nextCursor = cursor;

  for (const event of page) {
    if (event.run_id !== runId) return { events: existing, cursor, valid: false };
    if (known.has(event.sequence) || (nextCursor !== null && event.sequence <= nextCursor)) continue;

    const expected = nextCursor === null ? 0 : nextCursor + 1;
    if (event.sequence !== expected) return { events: existing, cursor, valid: false };

    merged.push(event);
    known.add(event.sequence);
    nextCursor = event.sequence;
  }

  return { events: merged, cursor: nextCursor, valid: true };
}

export function activityFor(events: ServiceEvent[], fallback: ActivityLabel): ActivityLabel {
  const latest = events.at(-1);
  if (latest === undefined) return fallback;
  if (latest.category === "knowledge") return "Searching knowledge…";
  if (latest.category === "model") return "Thinking…";
  if (latest.category === "approval") {
    if (latest.phase === "suspended" || latest.phase === "prepared") return "Waiting for decision…";
    if (latest.phase === "resumed" || latest.phase === "granted") return "Resuming…";
    return "Approval required";
  }
  if (latest.graph_node_id?.toLowerCase().includes("verify")) return "Verifying…";
  if (latest.category === "graph" || latest.category === "run") return "Finishing…";
  return fallback;
}

function uniqueIds(events: ServiceEvent[], category: ServiceEvent["category"], phase = "started") {
  return [...new Set(
    events
      .filter((event) => event.category === category && event.phase === phase)
      .map((event) => event.correlation_id)
      .filter((id): id is string => id !== null),
  )];
}

function latestBudget(events: ServiceEvent[], category: ServiceEvent["category"]) {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event?.category === category && event.budget_usage !== null && event.budget_limit !== null) {
      return { usage: event.budget_usage, limit: event.budget_limit };
    }
  }
  return null;
}

function phaseState(
  events: ServiceEvent[],
  category: ServiceEvent["category"],
  active: boolean,
): TimelineItem["state"] {
  const matching = events.filter((event) => event.category === category);
  if (matching.some(({ phase }) => phase === "failed")) return "failed";
  if (matching.some(({ phase }) => phase === "completed" || phase === "bound")) return "completed";
  if (active && matching.length > 0) return "active";
  return "pending";
}

function timelineFor(events: ServiceEvent[], status: RunView, active: boolean): TimelineItem[] {
  const verifyEvents = events.filter((event) => event.graph_node_id?.toLowerCase().includes("verify"));
  const terminal = status.disposition === "completed"
    || status.disposition === "failed"
    || status.disposition === "manual_reconciliation_required";
  const terminalState: TimelineItem["state"] = !terminal
    ? "pending"
    : status.outcome === "cancelled"
      ? "cancelled"
      : status.disposition === "completed"
        ? "completed"
        : "failed";

  return [
    { id: "retrieve", label: "Retrieve", state: phaseState(events, "knowledge", active) },
    { id: "model", label: "Model", state: phaseState(events, "model", active) },
    {
      id: "verify",
      label: "Verify",
      state: verifyEvents.some(({ phase }) => phase === "failed")
        ? "failed"
        : verifyEvents.some(({ phase }) => phase === "completed")
          ? "completed"
          : active && verifyEvents.length > 0
            ? "active"
            : "pending",
    },
    { id: "complete", label: "Complete", state: terminalState },
  ];
}

export function runDetails(
  status: RunView,
  events: ServiceEvent[],
  startedAtMillis: number,
  nowMillis: number,
  activity: ActivityLabel | null,
): RunDetails {
  const retrievalBackends = [...new Set(events.flatMap(({ knowledge_backends }) => knowledge_backends))];
  const retrievalIds = uniqueIds(events, "knowledge");
  const modelIds = uniqueIds(events, "model");
  const terminal = status.disposition === "completed"
    || status.disposition === "failed"
    || status.disposition === "manual_reconciliation_required";
  const elapsedMillis = status.duration_millis ?? Math.max(0, nowMillis - startedAtMillis);
  const currentPhase = terminal
    ? status.disposition === "manual_reconciliation_required"
      ? "Reconciliation required"
      : status.outcome === "cancelled"
        ? "Cancelled"
        : status.disposition === "completed" ? "Complete" : "Failed"
    : (activity ?? "Starting…").replace("…", "");

  return {
    runId: status.run_id,
    workflow: status.workflow_id ?? "Unavailable",
    status: status.disposition,
    currentPhase,
    elapsedMillis,
    modelCount: modelIds.length,
    modelBudget: latestBudget(events, "model"),
    graphBudget: latestBudget(events, "graph"),
    retrievalBackends,
    evidenceCount: null,
    citationCount: status.result?.kind === "final_answer" ? status.result.citations.length : 0,
    retrievalIds,
    modelIds,
    terminalStatus: terminal
      ? status.disposition === "manual_reconciliation_required"
        ? status.disposition
        : (status.outcome ?? status.disposition)
      : null,
    timeline: timelineFor(events, status, !terminal),
  };
}

export function formatDuration(milliseconds: number) {
  if (milliseconds < 1_000) return `${milliseconds}ms`;
  return `${(milliseconds / 1_000).toFixed(1)}s`;
}
