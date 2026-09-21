import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Command,
  Monitor,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Search,
  Sun,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";

import { CommandPalette, type PaletteCommand } from "@/app/CommandPalette";
import { useTheme, type ThemePreference } from "@/app/ThemeContext";
import { ConversationWorkspace } from "@/components/ConversationWorkspace";
import { ReadinessInspector } from "@/components/ReadinessInspector";
import { SessionSidebar } from "@/components/SessionSidebar";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { serviceQueryKeys, useServiceState } from "@/queries/service";

const nextTheme: Record<ThemePreference, ThemePreference> = {
  system: "light",
  light: "dark",
  dark: "system",
};

const themeIcon = { system: Monitor, light: Sun, dark: Moon } as const;

function isEditableTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

function ThemeButton() {
  const { preference, setPreference } = useTheme();
  const Icon = themeIcon[preference];
  const label = `Theme: ${preference}. Activate to change theme.`;

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      aria-label={label}
      title={label}
      onClick={() => setPreference(nextTheme[preference])}
    >
      <Icon aria-hidden="true" className="size-4" />
    </Button>
  );
}

export function App() {
  const queryClient = useQueryClient();
  const reduceMotion = useReducedMotion();
  const { health, readiness, version } = useServiceState();
  const [sidebarOpen, setSidebarOpen] = useState(() => window.matchMedia("(min-width: 821px)").matches);
  const [inspectorOpen, setInspectorOpen] = useState(() => window.matchMedia("(min-width: 1041px)").matches);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);

  const connected = health.isSuccess;
  const draining = health.data?.lifecycle === "draining";
  const readinessStatus = readiness.data?.overall ?? "unavailable";
  const serviceState = !connected ? "unavailable" : draining ? "draining" : readinessStatus;

  const newTask = useCallback(() => {
    setSelectedSessionId(null);
    setPaletteOpen(false);
  }, []);

  const refresh = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: serviceQueryKeys.all });
  }, [queryClient]);

  const commands = useMemo<PaletteCommand[]>(
    () => [
      { id: "new-task", label: "New task", shortcut: "Ctrl N", icon: Command, action: newTask },
      {
        id: "sidebar",
        label: sidebarOpen ? "Collapse conversations" : "Expand conversations",
        shortcut: "Ctrl B",
        icon: sidebarOpen ? PanelLeftClose : PanelLeftOpen,
        action: () => setSidebarOpen((value) => !value),
      },
      {
        id: "inspector",
        label: inspectorOpen ? "Hide inspector" : "Show inspector",
        shortcut: "Ctrl Shift I",
        icon: inspectorOpen ? PanelRightClose : PanelRightOpen,
        action: () => setInspectorOpen((value) => !value),
      },
      { id: "refresh", label: "Refresh service status", icon: Search, action: () => void refresh() },
    ],
    [inspectorOpen, newTask, refresh, sidebarOpen],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
      const key = event.key.toLowerCase();

      if (key === "k") {
        event.preventDefault();
        setPaletteOpen((value) => !value);
        return;
      }
      if (isEditableTarget(event.target)) return;

      if (key === "n" && !event.shiftKey) {
        event.preventDefault();
        newTask();
      } else if (key === "b" && !event.shiftKey) {
        event.preventDefault();
        setSidebarOpen((value) => !value);
      } else if (key === "i" && event.shiftKey) {
        event.preventDefault();
        setInspectorOpen((value) => !value);
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [newTask]);

  return (
    <div className="grid h-dvh min-h-[520px] grid-rows-[40px_minmax(0,1fr)_22px] overflow-hidden bg-background text-foreground">
      <header className="flex items-center justify-between border-b border-border bg-panel px-2.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={sidebarOpen ? "Collapse conversations" : "Expand conversations"}
            title="Toggle conversations (Ctrl+B)"
            aria-pressed={sidebarOpen}
            onClick={() => setSidebarOpen((value) => !value)}
          >
            {sidebarOpen ? <PanelLeftClose aria-hidden="true" /> : <PanelLeftOpen aria-hidden="true" />}
          </Button>
          <span className="mx-1 h-4 w-px bg-border" aria-hidden="true" />
          <h1 className="truncate text-[13px] font-semibold">Enterprise Local Agent</h1>
          <span className="hidden truncate text-xs text-muted sm:inline">/ Local workspace</span>
        </div>

        <button
          type="button"
          className="absolute left-1/2 hidden h-7 w-64 -translate-x-1/2 items-center gap-2 rounded-control border border-border bg-background px-2.5 text-xs text-muted outline-none transition-colors hover:bg-selection hover:text-secondary focus-visible:ring-2 focus-visible:ring-focus lg:flex"
          onClick={() => setPaletteOpen(true)}
          aria-label="Open command palette"
        >
          <Search aria-hidden="true" className="size-3.5" />
          <span>Search commands</span>
          <kbd className="ml-auto">Ctrl K</kbd>
        </button>

        <div className="flex items-center gap-0.5">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}
            title="Toggle inspector (Ctrl+Shift+I)"
            aria-pressed={inspectorOpen}
            onClick={() => setInspectorOpen((value) => !value)}
          >
            {inspectorOpen ? <PanelRightClose aria-hidden="true" /> : <PanelRightOpen aria-hidden="true" />}
          </Button>
          <ThemeButton />
        </div>
      </header>

      <div className="relative flex min-h-0 min-w-0 overflow-hidden">
        <motion.div
          className="sidebar-pane relative z-20 shrink-0 overflow-hidden border-r border-border bg-panel"
          initial={false}
          animate={{ width: sidebarOpen ? 244 : 48 }}
          transition={reduceMotion ? { duration: 0 } : { duration: 0.16, ease: "easeOut" }}
        >
          <SessionSidebar
            expanded={sidebarOpen}
            selectedSessionId={selectedSessionId}
            onSelectSession={setSelectedSessionId}
            onNewTask={newTask}
          />
        </motion.div>

        <main className="min-w-0 flex-1 bg-background">
          <ConversationWorkspace
            selectedSessionId={selectedSessionId}
            serviceState={serviceState}
            onNewTask={newTask}
            onRetry={() => void refresh()}
          />
        </main>

        <AnimatePresence initial={false}>
          {inspectorOpen && (
            <motion.div
              className="inspector-pane z-30 w-[300px] shrink-0 overflow-hidden border-l border-border bg-panel"
              initial={reduceMotion ? false : { x: 18, opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={reduceMotion ? { opacity: 0 } : { x: 18, opacity: 0 }}
              transition={{ duration: reduceMotion ? 0 : 0.14, ease: "easeOut" }}
            >
              <ReadinessInspector
                state={serviceState}
                readiness={readiness.data}
                version={version.data}
                onClose={() => setInspectorOpen(false)}
              />
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <footer className="flex items-center justify-between border-t border-border bg-panel px-2.5 text-[11px] text-muted">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className={cn("status-dot", `status-dot-${serviceState}`)} aria-hidden="true" />
          <span className="truncate">
            {serviceState === "ready" && "Local service ready"}
            {serviceState === "degraded" && "Local service degraded"}
            {serviceState === "unavailable" && "Local service unavailable"}
            {serviceState === "draining" && "Local service draining"}
          </span>
        </div>
        <div className="flex items-center gap-3">
          <button
            type="button"
            className="outline-none hover:text-foreground focus-visible:text-foreground focus-visible:underline"
            onClick={() => setPaletteOpen(true)}
          >
            Commands
          </button>
          <span>{version.data === undefined ? "Version unavailable" : `v${version.data.version}`}</span>
        </div>
      </footer>

      <CommandPalette open={paletteOpen} commands={commands} onOpenChange={setPaletteOpen} />
      <div className="sr-only" aria-live="polite">Service state: {serviceState}</div>
    </div>
  );
}
