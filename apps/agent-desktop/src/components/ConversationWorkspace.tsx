import { ArrowUp, CircleAlert, FileText, Plus, RefreshCw, Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";

type ServiceState = "ready" | "degraded" | "unavailable" | "draining";

interface ConversationWorkspaceProps {
  selectedSessionId: string | null;
  serviceState: ServiceState;
  onNewTask: () => void;
  onRetry: () => void;
}

function Composer({ unavailable }: { unavailable: boolean }) {
  return (
    <div className="mx-auto w-full max-w-3xl px-4 pb-4">
      <div className="rounded-composer border border-border-strong bg-panel focus-within:border-focus focus-within:ring-1 focus-within:ring-focus">
        <label htmlFor="task-composer" className="sr-only">Task composer</label>
        <textarea
          id="task-composer"
          rows={2}
          readOnly
          aria-describedby="composer-status"
          placeholder={unavailable ? "Local service unavailable" : "Describe a task for your enterprise agent"}
          className="block max-h-36 min-h-16 w-full resize-none bg-transparent px-3.5 pt-3 text-sm leading-5 outline-none placeholder:text-muted"
        />
        <div className="flex h-10 items-center justify-between px-2">
          <Button variant="ghost" size="icon" aria-label="Attach context unavailable" title="Attachments are not available yet" disabled>
            <Plus aria-hidden="true" className="size-4" />
          </Button>
          <div className="flex items-center gap-2">
            <span id="composer-status" className="text-[11px] text-muted">Execution arrives in a later milestone</span>
            <Button variant="primary" size="icon" aria-label="Send task unavailable" disabled>
              <ArrowUp aria-hidden="true" className="size-4" />
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function ConversationWorkspace({ selectedSessionId, serviceState, onNewTask, onRetry }: ConversationWorkspaceProps) {
  const unavailable = serviceState === "unavailable";

  return (
    <section aria-label="Task workspace" className="flex h-full min-w-0 flex-col">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
        <h2 className="truncate text-xs font-medium">{selectedSessionId ? "Conversation preview" : "New task"}</h2>
        {serviceState !== "ready" && (
          <span className="flex items-center gap-1.5 text-[11px] capitalize text-muted">
            <span className={`status-dot status-dot-${serviceState}`} aria-hidden="true" />
            {serviceState}
          </span>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {unavailable ? (
          <div className="grid min-h-full place-items-center px-6 py-10">
            <div className="max-w-sm text-center">
              <CircleAlert aria-hidden="true" className="mx-auto mb-3 size-6 text-danger" />
              <h3 className="text-base font-semibold">Local service unavailable</h3>
              <p className="mt-1.5 text-sm leading-6 text-secondary">Tasks stay disabled until the trusted local service can be reached.</p>
              <Button className="mt-4" onClick={onRetry}>
                <RefreshCw aria-hidden="true" className="size-3.5" />
                Retry connection
              </Button>
            </div>
          </div>
        ) : selectedSessionId ? (
          <div className="mx-auto max-w-3xl px-6 py-12">
            <div className="flex gap-3 border-b border-border pb-6">
              <div className="grid size-7 shrink-0 place-items-center rounded-control border border-border bg-panel">
                <FileText aria-hidden="true" className="size-3.5 text-secondary" />
              </div>
              <div>
                <p className="text-sm font-medium">Conversation layout preview</p>
                <p className="mt-1 text-sm leading-6 text-secondary">Historical conversation content is intentionally not wired in M16.2.</p>
              </div>
            </div>
          </div>
        ) : (
          <div className="grid min-h-full place-items-center px-6 py-10">
            <div className="w-full max-w-lg text-center">
              <div className="mx-auto mb-4 grid size-10 place-items-center rounded-control border border-border bg-panel">
                <Sparkles aria-hidden="true" className="size-4 text-accent" />
              </div>
              <h3 className="text-lg font-semibold">What are you working on?</h3>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-secondary">Start a focused engineering task grounded in your trusted local knowledge.</p>
              <button
                type="button"
                className="mt-5 text-xs font-medium text-accent outline-none hover:underline focus-visible:ring-2 focus-visible:ring-focus"
                onClick={onNewTask}
              >
                New task · Ctrl N
              </button>
            </div>
          </div>
        )}
      </div>

      <Composer unavailable={unavailable} />
    </section>
  );
}
