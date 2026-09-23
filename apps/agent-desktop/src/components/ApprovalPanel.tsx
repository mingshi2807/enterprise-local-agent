import { Check, FilePenLine, LoaderCircle, ShieldAlert, X } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import type { ApprovalUiState } from "@/queries/conversation";

interface Props {
  approval: ApprovalUiState;
  onApprove: () => Promise<unknown>;
  onDeny: () => Promise<unknown>;
  onResume: () => Promise<unknown>;
  onAbort: () => Promise<unknown>;
}

const errorText = {
  unauthorized: "This identity cannot decide or resume this approval. A separate authorized approver may be required.",
  stale: "This approval changed on the service. Refresh before deciding again.",
  unavailable: "Approval details are temporarily unavailable.",
} as const;

export function ApprovalPanel({ approval, onApprove, onDeny, onResume, onAbort }: Props) {
  const { item, preview, loadingPreview, busy, error } = approval;
  const [commandError, setCommandError] = useState<ApprovalUiState["error"]>(null);
  const decided = item.state === "approved" || item.state === "denied";
  const visibleError = commandError ?? error;

  const run = async (command: () => Promise<unknown>) => {
    setCommandError(null);
    try {
      await command();
    } catch (cause) {
      if (typeof cause === "object" && cause !== null && "code" in cause) {
        if (cause.code === "unauthorized") {
          setCommandError("unauthorized");
          return;
        }
        if (cause.code === "state_conflict") {
          setCommandError("stale");
          return;
        }
      }
      setCommandError("unavailable");
    }
  };

  return (
    <section className="approval-panel" aria-labelledby="approval-title">
      <div className="flex min-w-0 items-start gap-2.5">
        <FilePenLine aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-3">
            <h3 id="approval-title" className="text-xs font-semibold">LocalWrite approval required</h3>
            <span className="rounded-control border border-border px-1.5 py-0.5 text-[11px] capitalize text-secondary" aria-live="polite">
              {item.state === "approved" ? "Ready to resume" : item.state}
            </span>
          </div>
          {loadingPreview && <p className="mt-2 text-xs text-muted" role="status">Loading trusted preview…</p>}
          {preview !== null && (
            <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
              <dt className="text-muted">Operation</dt><dd>{preview.operation}</dd>
              <dt className="text-muted">Target</dt><dd className="truncate font-mono" title={preview.target}>{preview.target}</dd>
              <dt className="text-muted">Content</dt><dd>{preview.content_bytes.toLocaleString()} bytes</dd>
            </dl>
          )}
          {visibleError !== null && (
            <p className="mt-2 flex gap-1.5 text-xs text-danger" role="alert">
              <ShieldAlert aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />{errorText[visibleError]}
            </p>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2">
            {item.state === "waiting" && (
              <>
                <Button className="h-7 px-2 text-xs" variant="primary" disabled={busy || preview === null} onClick={() => void run(onApprove)}>
                  <Check aria-hidden="true" className="size-3.5" />Approve
                </Button>
                <Button className="h-7 px-2 text-xs" variant="secondary" disabled={busy || preview === null} onClick={() => void run(onDeny)}>
                  <X aria-hidden="true" className="size-3.5" />Deny
                </Button>
                <Button className="h-7 px-2 text-xs" variant="ghost" disabled={busy} onClick={() => void run(onAbort)}>Abort run</Button>
              </>
            )}
            {decided && (
              <>
                <Button className="h-7 px-2 text-xs" variant="primary" disabled={busy} onClick={() => void run(onResume)}>
                  {busy && <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin" />}
                  {item.state === "approved" ? "Resume approved run" : "Finish denied run"}
                </Button>
              </>
            )}
            {item.state === "executing" && <span className="text-xs text-muted">Contained execution is in progress.</span>}
            {busy && item.state === "waiting" && <span className="text-[11px] text-muted" role="status">Recording decision…</span>}
          </div>
        </div>
      </div>
    </section>
  );
}
