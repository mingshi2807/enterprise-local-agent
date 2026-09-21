import { useEffect, useRef, useState } from "react";
import { ArrowUp, CircleAlert, LoaderCircle, RefreshCw, Square, Sparkles } from "lucide-react";

import { MarkdownAnswer } from "@/components/MarkdownAnswer";
import { Button } from "@/components/ui/button";
import type { Conversation } from "@/queries/conversation";

type ServiceState = "ready" | "degraded" | "unavailable" | "draining";

interface Props {
  conversation: Conversation | null;
  serviceState: ServiceState;
  readonlyReady: boolean;
  sending: boolean;
  cancelling: boolean;
  focusNonce: number;
  onSend: (prompt: string) => Promise<unknown>;
  onCancel: () => Promise<unknown>;
  onNewTask: () => void;
  onRetryRun: (() => Promise<unknown>) | null;
  onRetryConnection: () => void;
}

function Composer({ disabled, busy, focusNonce, onSend }: Pick<Props, "focusNonce" | "onSend"> & { disabled: boolean; busy: boolean }) {
  const [text, setText] = useState("");
  const [submissionError, setSubmissionError] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const bytes = new TextEncoder().encode(text).byteLength;
  const valid = text.trim().length > 0 && bytes <= 8 * 1024 && !disabled && !busy;

  useEffect(() => textarea.current?.focus(), [focusNonce]);

  const submit = async () => {
    if (!valid) return;
    const prompt = text.trim();
    setText("");
    setSubmissionError(false);
    try {
      await onSend(prompt);
    } catch {
      setText(prompt);
      setSubmissionError(true);
    }
  };

  return (
    <div className="mx-auto w-full max-w-3xl px-4 pb-4">
      {submissionError && <p className="mb-2 text-xs text-danger" role="alert">The local agent service could not start this task.</p>}
      <div className="rounded-composer border border-border-strong bg-panel focus-within:border-focus focus-within:ring-1 focus-within:ring-focus">
        <label htmlFor="task-composer" className="sr-only">Task composer</label>
        <textarea
          ref={textarea}
          id="task-composer"
          rows={2}
          value={text}
          disabled={disabled}
          aria-describedby="composer-status"
          placeholder={disabled ? "ReadOnly workflow unavailable" : "Ask about your enterprise engineering knowledge"}
          className="block max-h-40 min-h-18 w-full resize-none bg-transparent px-3.5 pt-3 text-sm leading-6 outline-none placeholder:text-muted disabled:cursor-not-allowed disabled:opacity-60"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void submit();
            }
          }}
        />
        <div className="flex min-h-10 items-center justify-end gap-2 px-2">
          <span id="composer-status" className={bytes > 8 * 1024 ? "text-[11px] text-danger" : "text-[11px] text-muted"}>
            {bytes > 7 * 1024 ? `${bytes.toLocaleString()} / 8,192 bytes` : "Ctrl Enter to send"}
          </span>
          <Button variant="primary" size="icon" aria-label="Send task" title="Send task (Ctrl+Enter)" disabled={!valid} onClick={() => void submit()}>
            {busy ? <LoaderCircle aria-hidden="true" className="size-4 animate-spin" /> : <ArrowUp aria-hidden="true" className="size-4" />}
          </Button>
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
  const { conversation, serviceState, readonlyReady, sending, cancelling, focusNonce, onSend, onCancel, onNewTask, onRetryRun, onRetryConnection } = props;
  const scrollArea = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  const active = conversation?.activeRun ?? null;
  const unavailable = serviceState === "unavailable";

  useEffect(() => {
    const area = scrollArea.current;
    if (!followTail.current || area === null) return;
    if (typeof area.scrollTo === "function") {
      area.scrollTo({ top: area.scrollHeight, behavior: "smooth" });
    } else {
      area.scrollTop = area.scrollHeight;
    }
  }, [conversation?.messages.length, active?.activity]);

  return (
    <section aria-label="Task workspace" className="flex h-full min-w-0 flex-col">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
        <h2 className="truncate text-xs font-medium">{conversation?.title ?? "New task"}</h2>
        <div className="flex items-center gap-2">
          {active !== null && (
            <Button variant="secondary" className="h-7 px-2 text-xs" disabled={cancelling} onClick={() => void onCancel()}>
              <Square aria-hidden="true" className="size-3" /> Stop
            </Button>
          )}
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
          followTail.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
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
              <article key={message.id} className={`message-row message-${message.role}`}>
                <div className="message-label">{message.role === "user" ? "You" : message.role === "assistant" ? "Agent" : "Run status"}</div>
                {message.role === "assistant" ? (
                  <><MarkdownAnswer answer={message.content} /><Citations citations={message.citations ?? []} /></>
                ) : (
                  <p className="whitespace-pre-wrap text-sm leading-6">{message.content}</p>
                )}
                {message.role === "error" && onRetryRun !== null && message === conversation.messages.at(-1) && message.errorKind !== "cancelled" && (
                  <Button className="mt-3 h-7 px-2 text-xs" variant="secondary" onClick={() => void onRetryRun()?.catch(() => undefined)}><RefreshCw aria-hidden="true" className="size-3.5" />Retry as new run</Button>
                )}
              </article>
            ))}
            {active !== null && <div className="activity-row" role="status" aria-live="polite"><LoaderCircle aria-hidden="true" className="size-3.5 animate-spin text-accent" />{active.activity}</div>}
          </div>
        )}
      </div>

      <Composer disabled={!readonlyReady || active !== null} busy={sending} focusNonce={focusNonce} onSend={onSend} />
    </section>
  );
}
