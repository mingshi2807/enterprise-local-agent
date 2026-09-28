import { memo } from "react";
import { Archive, CircleAlert, Clock3, LoaderCircle, MessageSquareText, Plus, ShieldAlert } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import type { Conversation } from "@/queries/conversation";

interface Props {
  expanded: boolean;
  conversations: Conversation[];
  loading: boolean;
  error: boolean;
  selectedSessionId: string | null;
  onSelectSession: (id: string) => void;
  onNewTask: () => void;
  onArchive: (id: string) => void;
  archivingSessionId: string | null;
}

function canArchive(conversation: Conversation) {
  return conversation.activeRun === null
    && conversation.approval === null
    && conversation.lastRun?.disposition !== "waiting"
    && conversation.lastRun?.disposition !== "resumable"
    && conversation.lastRun?.disposition !== "manual_reconciliation_required";
}

function statusLabel(conversation: Conversation) {
  if (conversation.approval !== null || conversation.lastRun?.disposition === "waiting") return "Approval required";
  if (conversation.lastRun?.disposition === "manual_reconciliation_required") return "Reconciliation required";
  if (conversation.lastRun?.outcome === "cancelled") return "Cancelled";
  if (conversation.lastRun?.disposition === "failed") return "Failed";
  if (conversation.activeRun !== null) return conversation.activeRun.activity;
  if (conversation.lastRun?.disposition === "completed") return "Completed";
  return conversation.lastActivityMillis === null
    ? "No runs yet"
    : new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })
        .format(new Date(conversation.lastActivityMillis));
}

export const SessionSidebar = memo(function SessionSidebar({ expanded, conversations, loading, error, selectedSessionId, onSelectSession, onNewTask, onArchive, archivingSessionId }: Props) {
  if (!expanded) {
    return (
      <nav aria-label="Conversation navigation" className="flex h-full w-12 flex-col items-center gap-1 py-2">
        <Button variant="ghost" size="icon" aria-label="New task" title="New task (Ctrl+N)" onClick={onNewTask}><Plus aria-hidden="true" className="size-4" /></Button>
        <Button variant="ghost" size="icon" aria-label="Conversation history" title="Conversation history" disabled={conversations.length === 0}><MessageSquareText aria-hidden="true" className="size-4" /></Button>
      </nav>
    );
  }

  return (
    <aside aria-label="Conversations" className="flex h-full w-[244px] flex-col">
      <div className="flex h-12 items-center border-b border-border px-2">
        <Button className="min-w-0 flex-1 justify-start" variant="secondary" onClick={onNewTask}><Plus aria-hidden="true" className="size-3.5" />New task<kbd className="ml-auto text-[10px] text-muted">Ctrl N</kbd></Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-1.5 py-2">
        <div className="flex items-center gap-1.5 px-2 pb-1.5 text-[11px] font-medium text-muted"><Clock3 aria-hidden="true" className="size-3" />Recent</div>
        {loading ? <p className="px-2 py-3 text-xs leading-5 text-muted">Loading conversations…</p> : error ? (
          <p className="px-2 py-3 text-xs leading-5 text-danger">Conversation history unavailable.</p>
        ) : conversations.length === 0 ? <p className="px-2 py-3 text-xs leading-5 text-muted">No conversations yet.</p> : (
          <nav aria-label="Recent conversations" className="space-y-0.5">
            {conversations.map((conversation) => (
              <div key={conversation.sessionId} className={cn("conversation-history-row group flex w-full items-center rounded-control transition-colors hover:bg-selection", selectedSessionId === conversation.sessionId && "bg-selection")}>
                <button type="button" className="min-w-0 flex-1 px-2 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-focus" aria-current={selectedSessionId === conversation.sessionId ? "page" : undefined} onClick={() => onSelectSession(conversation.sessionId)}>
                  <span className="flex items-center gap-1.5">
                    <span className="block min-w-0 flex-1 truncate text-xs font-medium">{conversation.title}</span>
                    {conversation.approval !== null || conversation.lastRun?.disposition === "waiting" ? <ShieldAlert aria-label="Approval required" className="size-3 shrink-0 text-warning" /> : null}
                    {conversation.lastRun?.disposition === "manual_reconciliation_required" || (conversation.lastRun?.disposition === "failed" && conversation.lastRun.outcome !== "cancelled") ? <CircleAlert aria-label="Run needs attention" className="size-3 shrink-0 text-danger" /> : null}
                    {conversation.activeRun !== null && conversation.lastRun?.disposition !== "waiting" && <LoaderCircle aria-label="Run active" className="size-3 shrink-0 animate-spin text-accent" />}
                  </span>
                  <span className="mt-0.5 block truncate text-[11px] text-muted">{statusLabel(conversation)}</span>
                </button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mr-1 size-7 shrink-0 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
                  aria-label="Remove from history"
                  title={canArchive(conversation) ? "Remove from history" : "Finish this conversation before removing it"}
                  disabled={!canArchive(conversation) || archivingSessionId === conversation.sessionId}
                  onClick={() => onArchive(conversation.sessionId)}
                >
                  {archivingSessionId === conversation.sessionId
                    ? <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin" />
                    : <Archive aria-hidden="true" className="size-3.5" />}
                </Button>
              </div>
            ))}
          </nav>
        )}
      </div>
    </aside>
  );
});
