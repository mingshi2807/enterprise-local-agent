import { Check, Circle, CircleAlert, Copy, LoaderCircle, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { BuildInfo, Readiness, ServiceConnectionState } from "@/bridge/contracts";
import { Button } from "@/components/ui/button";
import { formatDuration, runDetails, type TimelineItem } from "@/features/runActivity";
import { cn } from "@/lib/cn";
import type { Conversation } from "@/queries/conversation";

interface RunInspectorProps {
  state: ServiceConnectionState;
  readiness?: Readiness;
  version?: BuildInfo;
  conversation: Conversation | null;
  nowMillis: number;
  onClose: () => void;
}

function CopyableId({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | null>(null);
  useEffect(() => () => {
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
  }, []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
      resetTimer.current = window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="grid grid-cols-[5rem_minmax(0,1fr)_1.75rem] items-center gap-2 py-1 text-xs">
      <span className="text-muted">{label}</span>
      <code className="truncate text-[11px] text-secondary" title={value}>{value}</code>
      <Button type="button" variant="ghost" size="icon" aria-label={copied ? `${label} copied` : `Copy ${label}`} title={copied ? "Copied" : `Copy ${label}`} onClick={() => void copy()}>
        {copied ? <Check aria-hidden="true" className="size-3 text-success" /> : <Copy aria-hidden="true" className="size-3" />}
      </Button>
    </div>
  );
}

function TimelineIcon({ state }: { state: TimelineItem["state"] }) {
  if (state === "active") return <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin text-accent" />;
  if (state === "completed") return <Check aria-hidden="true" className="size-3.5 text-success" />;
  if (state === "failed") return <CircleAlert aria-hidden="true" className="size-3.5 text-danger" />;
  if (state === "cancelled") return <Circle aria-hidden="true" className="size-3.5 text-muted" />;
  return <Circle aria-hidden="true" className="size-3 text-border-strong" />;
}

export function RunInspector({ state, readiness, version, conversation, nowMillis, onClose }: RunInspectorProps) {
  const closeButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    closeButton.current?.focus();
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);
  const active = conversation?.activeRun ?? null;
  const status = conversation?.lastRun ?? null;
  const events = active?.events ?? conversation?.lastRunEvents ?? [];
  const startedAt = active?.startedAtMillis ?? conversation?.lastRunStartedAtMillis ?? nowMillis;
  const details = status === null ? null : runDetails(status, events, startedAt, nowMillis, active?.activity ?? null);

  return (
    <aside aria-labelledby="inspector-title" className="flex h-full w-[300px] flex-col">
      <div className="flex h-10 items-center justify-between border-b border-border px-3">
        <h2 id="inspector-title" className="text-xs font-semibold">Run details</h2>
        <Button ref={closeButton} variant="ghost" size="icon" aria-label="Close inspector" onClick={onClose}>
          <X aria-hidden="true" className="size-3.5" />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {details === null ? (
          <div className="px-3 py-5">
            <p className="text-xs font-medium">No run selected</p>
            <p className="mt-1 text-xs leading-5 text-muted">Run metadata appears here after a task starts.</p>
          </div>
        ) : (
          <>
            <section className="border-b border-border px-3 py-3" aria-labelledby="overview-title">
              <div className="flex items-center justify-between gap-3">
                <h3 id="overview-title" className="inspector-heading">Overview</h3>
                <span className="text-[11px] capitalize text-secondary">{details.status.replaceAll("_", " ")}</span>
              </div>
              <dl className="inspector-grid mt-3">
                <dt>Workflow</dt><dd title={details.workflow}>{details.workflow}</dd>
                <dt>Phase</dt><dd>{details.currentPhase}</dd>
                <dt>Elapsed</dt><dd>{formatDuration(details.elapsedMillis)}</dd>
                <dt>Model calls</dt><dd>{details.modelCount}</dd>
                <dt>Model budget</dt><dd>{details.modelBudget === null ? "—" : `${details.modelBudget.usage} / ${details.modelBudget.limit}`}</dd>
                <dt>Graph steps</dt><dd>{details.graphBudget === null ? "—" : `${details.graphBudget.usage} / ${details.graphBudget.limit}`}</dd>
                <dt>Backends</dt><dd>{details.retrievalBackends.length === 0 ? "—" : details.retrievalBackends.join(", ")}</dd>
                <dt>Evidence</dt><dd title="Not included in ServiceEventV2">Not exposed</dd>
                <dt>Citations</dt><dd>{details.citationCount}</dd>
                <dt>Terminal</dt><dd>{details.terminalStatus?.replaceAll("_", " ") ?? "—"}</dd>
              </dl>
            </section>

            <section className="border-b border-border px-3 py-3" aria-labelledby="timeline-title">
              <h3 id="timeline-title" className="inspector-heading">Activity</h3>
              <ol className="mt-3 space-y-0.5">
                {details.timeline.map((item) => (
                  <li key={item.id} className="grid grid-cols-[1rem_1fr] items-center gap-2 py-1.5 text-xs">
                    <TimelineIcon state={item.state} />
                    <span className={cn(item.state === "pending" ? "text-muted" : "text-secondary")}>{item.label}</span>
                  </li>
                ))}
              </ol>
            </section>

            <section className="border-b border-border px-3 py-3" aria-labelledby="identifiers-title">
              <h3 id="identifiers-title" className="inspector-heading">Identifiers</h3>
              <div className="mt-2">
                <CopyableId label="Run" value={details.runId} />
                {details.retrievalIds.map((id, index) => <CopyableId key={id} label={`Retrieval ${index + 1}`} value={id} />)}
                {details.modelIds.map((id, index) => <CopyableId key={id} label={`Model ${index + 1}`} value={id} />)}
              </div>
            </section>

            {conversation !== null && conversation.runHistory.length > 1 ? (
              <section className="border-b border-border px-3 py-3" aria-labelledby="history-title">
                <h3 id="history-title" className="inspector-heading">Previous runs</h3>
                <ol className="mt-2 space-y-1">
                  {conversation.runHistory.slice(1).map((run) => (
                    <li key={run.run_id} className="flex items-center justify-between gap-3 py-1 text-[11px]">
                      <code className="truncate text-muted" title={run.run_id}>{run.run_id.slice(0, 8)}</code>
                      <span className="shrink-0 capitalize text-secondary">{run.disposition.replaceAll("_", " ")}</span>
                    </li>
                  ))}
                </ol>
              </section>
            ) : null}
          </>
        )}

        <section className="px-3 py-3" aria-labelledby="service-section-title">
          <div className="flex items-center justify-between gap-3">
            <h3 id="service-section-title" className="inspector-heading">Local service</h3>
            <span className="flex items-center gap-1.5 text-[11px] capitalize text-secondary">
              <span className={cn("status-dot", `status-dot-${state}`)} aria-hidden="true" />{state}
            </span>
          </div>
          <dl className="inspector-grid mt-3">
            <dt>Application</dt><dd>{version?.application ?? "Unavailable"}</dd>
            <dt>Version</dt><dd>{version?.version ?? "—"}</dd>
            <dt>Workflow</dt><dd>{readiness?.workflows.find(({ workflow }) => workflow === details?.workflow)?.status ?? "—"}</dd>
          </dl>
        </section>
      </div>
    </aside>
  );
}
