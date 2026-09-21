import { Clock3, MessageSquareText, Plus, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

const sessions = [
  { id: "mock-1", title: "Review charging profile", time: "Today" },
  { id: "mock-2", title: "Summarize ISO requirements", time: "Yesterday" },
  { id: "mock-3", title: "Draft integration notes", time: "Sep 18" },
] as const;

interface SessionSidebarProps {
  expanded: boolean;
  selectedSessionId: string | null;
  onSelectSession: (id: string) => void;
  onNewTask: () => void;
}

export function SessionSidebar({ expanded, selectedSessionId, onSelectSession, onNewTask }: SessionSidebarProps) {
  if (!expanded) {
    return (
      <nav aria-label="Conversation navigation" className="flex h-full w-12 flex-col items-center gap-1 py-2">
        <Button variant="ghost" size="icon" aria-label="New task" title="New task (Ctrl+N)" onClick={onNewTask}>
          <Plus aria-hidden="true" className="size-4" />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Conversation history" title="Conversation history">
          <MessageSquareText aria-hidden="true" className="size-4" />
        </Button>
      </nav>
    );
  }

  return (
    <aside aria-label="Conversations" className="flex h-full w-[244px] flex-col">
      <div className="flex h-12 items-center gap-1.5 border-b border-border px-2">
        <Button className="min-w-0 flex-1 justify-start" variant="secondary" onClick={onNewTask}>
          <Plus aria-hidden="true" className="size-3.5" />
          New task
          <kbd className="ml-auto text-[10px] text-muted">Ctrl N</kbd>
        </Button>
        <Button variant="ghost" size="icon" aria-label="Search conversations" title="Search conversations" disabled>
          <Search aria-hidden="true" className="size-4" />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-1.5 py-2">
        <div className="flex items-center gap-1.5 px-2 pb-1.5 text-[11px] font-medium text-muted">
          <Clock3 aria-hidden="true" className="size-3" />
          Recent
        </div>
        <nav aria-label="Recent conversations" className="space-y-0.5">
          {sessions.map((session) => (
            <button
              key={session.id}
              type="button"
              className={cn(
                "group w-full rounded-control px-2 py-2 text-left outline-none transition-colors hover:bg-selection focus-visible:ring-2 focus-visible:ring-focus",
                selectedSessionId === session.id && "bg-selection",
              )}
              aria-current={selectedSessionId === session.id ? "page" : undefined}
              onClick={() => onSelectSession(session.id)}
            >
              <span className="block truncate text-xs font-medium">{session.title}</span>
              <span className="mt-0.5 block text-[11px] text-muted">{session.time} · Layout preview</span>
            </button>
          ))}
        </nav>
      </div>

      <div className="border-t border-border px-3 py-2 text-[10px] text-muted">Mock sessions for layout only</div>
    </aside>
  );
}
