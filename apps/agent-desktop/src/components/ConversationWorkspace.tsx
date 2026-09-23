import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, CircleAlert, LoaderCircle, RefreshCw, Square, Sparkles } from "lucide-react";
import { useReducedMotion } from "motion/react";

import { Button } from "@/components/ui/button";
import { formatDuration, runDetails } from "@/features/runActivity";
import type { ApprovalUiState, Conversation, WorkflowMode } from "@/queries/conversation";

const MarkdownAnswer = lazy(() =>
  import("@/components/MarkdownAnswer").then(({ MarkdownAnswer: component }) => ({ default: component })),
);
const ApprovalPanel = lazy(() =>
  import("@/components/ApprovalPanel").then(({ ApprovalPanel: component }) => ({ default: component })),
);

type ServiceState = "ready" | "degraded" | "unavailable" | "draining";
const MAX_INPUT_BYTES = 8 * 1024;

interface Props {
  conversation: Conversation | null;
  serviceState: ServiceState;
  readonlyReady: boolean;
  localWriteReady: boolean;
  sending: boolean;
  cancelling: boolean;
  focusNonce: number;
  nowMillis: number;
  approval: ApprovalUiState | null;
  onSend: (prompt: string, mode: WorkflowMode) => Promise<unknown>;
  onCancel: () => Promise<unknown>;
  onNewTask: () => void;
  onRetryRun: (() => Promise<unknown>) | null;
  onRetryConnection: () => void;
  onApprove: () => Promise<unknown>;
  onDeny: () => Promise<unknown>;
  onResume: () => Promise<unknown>;
  onAbort: () => Promise<unknown>;
}

function Composer({ workflowReady, running, stoppable, approvalRequired, busy, cancelling, focusNonce, onSend, onCancel, localWriteReady }: Pick<Props, "focusNonce" | "onSend" | "onCancel" | "localWriteReady" | "cancelling"> & { workflowReady: boolean; running: boolean; stoppable: boolean; approvalRequired: boolean; busy: boolean }) {
  const [text, setText] = useState("");
  const [mode, setMode] = useState<WorkflowMode>("readonly");
  const [submissionError, setSubmissionError] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const bytes = new TextEncoder().encode(text).byteLength;
  const valid = text.trim().length > 0
    && bytes <= MAX_INPUT_BYTES
    && workflowReady
    && (mode === "readonly" || localWriteReady)
    && !running
    && !busy;

  useEffect(() => textarea.current?.focus(), [focusNonce]);
  useLayoutEffect(() => {
    const element = textarea.current;
    if (element === null) return;
    element.style.height = "0px";
    const height = Math.min(Math.max(element.scrollHeight, 56), 160);
    element.style.height = `${height}px`;
    element.style.overflowY = element.scrollHeight > 160 ? "auto" : "hidden";
  }, [text]);

  const submit = async () => {
    if (!valid) return;
    const prompt = text.trim();
    setText("");
    setSubmissionError(false);
    try {
      await onSend(prompt, mode);
    } catch {
      setText(prompt);
      setSubmissionError(true);
    }
  };

  return (
    <div className="mx-auto w-full max-w-3xl px-4 pb-4">
      {submissionError && <p className="mb-2 text-xs text-danger" role="alert">The local agent service could not start this task.</p>}
      <div className="rounded-composer border border-border-strong bg-panel transition-colors focus-within:border-focus focus-within:ring-1 focus-within:ring-focus">
        <label htmlFor="task-composer" className="sr-only">Task composer</label>
        <textarea
          ref={textarea}
          id="task-composer"
          rows={1}
          value={text}
          disabled={!workflowReady || running}
          aria-describedby="composer-status"
          aria-invalid={bytes > MAX_INPUT_BYTES}
          placeholder={running ? "Run in progress" : !workflowReady ? "ReadOnly workflow unavailable" : "Ask about your enterprise engineering knowledge"}
          className="block min-h-14 w-full resize-none bg-transparent px-3.5 pt-3 text-sm leading-6 outline-none placeholder:text-muted disabled:cursor-not-allowed disabled:opacity-60"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void submit();
            }
          }}
        />
        <div className="flex min-h-10 items-center gap-2 px-2">
          <div className="flex items-center rounded-control border border-border p-0.5" aria-label="Workflow" role="group">
            <button type="button" className={`h-6 rounded-control px-2 text-[11px] ${mode === "readonly" ? "bg-selection text-foreground" : "text-muted"}`} aria-pressed={mode === "readonly"} onClick={() => setMode("readonly")}>Read only</button>
            <button type="button" disabled={!localWriteReady} className={`h-6 rounded-control px-2 text-[11px] disabled:opacity-40 ${mode === "localwrite" ? "bg-selection text-foreground" : "text-muted"}`} aria-pressed={mode === "localwrite"} title={localWriteReady ? "Allow a governed LocalWrite proposal" : "LocalWrite workflow unavailable"} onClick={() => setMode("localwrite")}>Local write</button>
          </div>
          <span id="composer-status" className={bytes > MAX_INPUT_BYTES ? "text-[11px] text-danger" : "text-[11px] text-muted"}>
            {approvalRequired ? "Approval required" : running ? "Run in progress" : bytes > 7 * 1024 ? `${bytes.toLocaleString()} / 8,192 bytes` : "Ctrl Enter to send"}
          </span>
          {stoppable ? (
            <Button className="ml-auto" variant="secondary" size="icon" aria-label={cancelling ? "Stopping" : "Stop"} title="Stop run" disabled={cancelling} onClick={() => void onCancel()}>
              {cancelling ? <LoaderCircle aria-hidden="true" className="size-4 animate-spin" /> : <Square aria-hidden="true" className="size-3.5" />}
            </Button>
          ) : !running ? (
            <Button className="ml-auto" variant="primary" size="icon" aria-label="Send task" title="Send task (Ctrl+Enter)" disabled={!valid} onClick={() => void submit()}>
              {busy ? <LoaderCircle aria-hidden="true" className="size-4 animate-spin" /> : <ArrowUp aria-hidden="true" className="size-4" />}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function Citations({ citations }: { citations: NonNullable<Conversation["messages"][number]["citations"]> }) {
  if (citations.length === 0) return null;
  return (
    <div className="mt-4 border-t border-border pt-3">
      <p className="mb-2 text-[11px] font-semibold uppercase text-muted">Sources</p>
      <div className="flex flex-wrap gap-1.5">
        {citations.map((citation, index) => (
          <details key={`${citation.evidence_id}-${index}`} className="citation-chip">
            <summary>[{index + 1}] {citation.backend} · {citation.reference_id}</summary>
            <div className="citation-detail">
              <div><span>Source</span>{citation.source_id}</div>
              <div><span>Reference</span>{citation.reference_id}</div>
              {citation.provenance !== null && <div><span>Provenance</span>{citation.provenance}</div>}
            </div>
          </details>
        ))}
      </div>
    </div>
  );
}

export function ConversationWorkspace(props: Props) {
  const { conversation, serviceState, readonlyReady, localWriteReady, approval, sending, cancelling, focusNonce, nowMillis, onSend, onCancel, onNewTask, onRetryRun, onRetryConnection, onApprove, onDeny, onResume, onAbort } = props;
  const scrollArea = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  const [atTail, setAtTail] = useState(true);
  const reduceMotion = useReducedMotion();
  const active = conversation?.activeRun ?? null;
  const unavailable = serviceState === "unavailable";

  const status = conversation?.lastRun ?? null;
  const events = active?.events ?? conversation?.lastRunEvents ?? [];
  const startedAt = active?.startedAtMillis ?? conversation?.lastRunStartedAtMillis ?? nowMillis;
  const details = status === null ? null : runDetails(status, events, startedAt, nowMillis, active?.activity ?? null);

  const scrollToEnd = useCallback((behavior: ScrollBehavior) => {
    const area = scrollArea.current;
    if (area === null) return;
    followTail.current = true;
    setAtTail(true);
    if (typeof area.scrollTo === "function") {
      area.scrollTo({ top: area.scrollHeight, behavior });
    } else {
      area.scrollTop = area.scrollHeight;
    }
  }, []);

  useEffect(() => {
    followTail.current = true;
    scrollToEnd("auto");
  }, [conversation?.sessionId, scrollToEnd]);

  useEffect(() => {
    if (!followTail.current) return;
    scrollToEnd(reduceMotion ? "auto" : "smooth");
  }, [active?.activity, conversation?.messages.length, reduceMotion, scrollToEnd]);

  return (
    <section aria-label="Task workspace" className="flex h-full min-w-0 flex-col">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
        <h2 className="truncate text-xs font-medium">{conversation?.title ?? "New task"}</h2>
        <div className="flex items-center gap-2">
          {serviceState !== "ready" && (
            <span className="flex items-center gap-1.5 text-[11px] capitalize text-muted">
              <span className={`status-dot status-dot-${serviceState}`} aria-hidden="true" />{serviceState}
            </span>
          )}
        </div>
      </div>

      <div
        ref={scrollArea}
        className="min-h-0 flex-1 overflow-auto"
        onScroll={(event) => {
          const element = event.currentTarget;
          const nextAtTail = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
          followTail.current = nextAtTail;
          setAtTail(nextAtTail);
        }}
      >
        {unavailable && conversation === null ? (
          <div className="grid min-h-full place-items-center px-6 py-10">
            <div className="max-w-sm text-center">
              <CircleAlert aria-hidden="true" className="mx-auto mb-3 size-6 text-danger" />
              <h3 className="text-base font-semibold">Local service unavailable</h3>
              <p className="mt-1.5 text-sm leading-6 text-secondary">Tasks stay disabled until the trusted local service can be reached.</p>
              <Button className="mt-4" onClick={onRetryConnection}><RefreshCw aria-hidden="true" className="size-3.5" />Retry connection</Button>
            </div>
          </div>
        ) : conversation === null ? (
          <div className="grid min-h-full place-items-center px-6 py-10">
            <div className="w-full max-w-lg text-center">
              <div className="mx-auto mb-4 grid size-10 place-items-center rounded-control border border-border bg-panel"><Sparkles aria-hidden="true" className="size-4 text-accent" /></div>
              <h3 className="text-lg font-semibold">What are you working on?</h3>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-secondary">Ask a focused engineering question grounded in trusted enterprise knowledge.</p>
              <button type="button" className="mt-5 text-xs font-medium text-accent outline-none hover:underline focus-visible:ring-2 focus-visible:ring-focus" onClick={onNewTask}>New task · Ctrl N</button>
            </div>
          </div>
        ) : (
          <div className="mx-auto w-full max-w-3xl px-5 py-6">
            {conversation.messages.map((message) => (
              <article key={message.id} className={`message-row message-${message.errorKind === "cancelled" ? "cancelled" : message.role}`}>
                <div className="message-label">{message.role === "user" ? "You" : message.role === "assistant" ? "Agent" : "Run status"}</div>
                {message.role === "assistant" ? (
                  <>
                    <Suspense fallback={<p className="text-sm text-muted">Rendering answer…</p>}>
                      <MarkdownAnswer answer={message.content} />
                    </Suspense>
                    <Citations citations={message.citations ?? []} />
                  </>
                ) : (
                  <p className="whitespace-pre-wrap text-sm leading-6">{message.content}</p>
                )}
                {message.role === "error" && onRetryRun !== null && message === conversation.messages.at(-1) && message.errorKind !== "cancelled" && (
                  <Button className="mt-3 h-7 px-2 text-xs" variant="secondary" onClick={() => void onRetryRun()?.catch(() => undefined)}><RefreshCw aria-hidden="true" className="size-3.5" />Retry as new run</Button>
                )}
              </article>
            ))}
            {approval !== null && (
              <Suspense fallback={<p className="mt-4 text-xs text-muted">Loading approval…</p>}>
                <ApprovalPanel approval={approval} onApprove={onApprove} onDeny={onDeny} onResume={onResume} onAbort={onAbort} />
              </Suspense>
            )}
            {active !== null && details !== null && (
              <div className="activity-row" role="status" aria-live="polite">
                <span className="activity-pulse" aria-hidden="true" />
                <span className="font-medium text-secondary">{active.activity}</span>
                <span className="text-muted">{formatDuration(details.elapsedMillis)}</span>
              </div>
            )}
            {active === null && details !== null && (
              <div className="run-summary" aria-label="Run summary">
                <span className={details.terminalStatus === "completed" ? "text-success" : details.terminalStatus === "cancelled" ? "text-muted" : "text-danger"}>
                  {details.terminalStatus === "completed"
                    ? "Completed"
                    : details.terminalStatus === "cancelled"
                      ? "Cancelled"
                      : details.terminalStatus === "manual_reconciliation_required"
                        ? "Reconciliation required"
                        : "Failed"}
                </span>
                <span aria-hidden="true">·</span>
                <span>{formatDuration(details.elapsedMillis)}</span>
                <span aria-hidden="true">·</span>
                <span>{details.citationCount} {details.citationCount === 1 ? "source" : "sources"}</span>
              </div>
            )}
            {!atTail && (
              <Button
                type="button"
                variant="secondary"
                className="sticky bottom-3 left-1/2 h-7 -translate-x-1/2 px-2 text-xs shadow-sm"
                onClick={() => scrollToEnd(reduceMotion ? "auto" : "smooth")}
              >
                <ArrowDown aria-hidden="true" className="size-3.5" />Jump to latest
              </Button>
            )}
          </div>
        )}
      </div>

      <Composer
        workflowReady={readonlyReady}
        running={active !== null}
        stoppable={active !== null && approval === null}
        approvalRequired={approval !== null}
        busy={sending}
        cancelling={cancelling}
        focusNonce={focusNonce}
        onSend={onSend}
        onCancel={onCancel}
        localWriteReady={localWriteReady}
      />
    </section>
  );
}
