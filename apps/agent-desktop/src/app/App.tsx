import {
  Activity,
  CircleAlert,
  Layers3,
  Monitor,
  Moon,
  RefreshCw,
  Sun,
  TerminalSquare,
} from "lucide-react";

import { useTheme, type ThemePreference } from "@/app/ThemeContext";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { serviceQueryKeys, useServiceState } from "@/queries/service";
import { useQueryClient } from "@tanstack/react-query";

const nextTheme: Record<ThemePreference, ThemePreference> = {
  system: "light",
  light: "dark",
  dark: "system",
};

const themeIcon = {
  system: Monitor,
  light: Sun,
  dark: Moon,
} as const;

function statusTone(status: "ready" | "degraded" | "unavailable") {
  return {
    ready: "bg-success",
    degraded: "bg-warning",
    unavailable: "bg-danger",
  }[status];
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
  const { health, readiness, version } = useServiceState();
  const connected = health.isSuccess;
  const status = readiness.data?.overall ?? "unavailable";
  const hasError = health.isError || readiness.isError || version.isError;

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: serviceQueryKeys.all });
  };

  return (
    <div className="grid h-dvh min-h-[640px] grid-rows-[44px_1fr_24px] overflow-hidden bg-background text-foreground">
      <header className="flex items-center justify-between border-b border-border bg-panel px-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <div className="grid size-7 place-items-center rounded-control bg-accent text-accent-foreground">
            <TerminalSquare aria-hidden="true" className="size-4" />
          </div>
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">Enterprise Local Agent</h1>
            <p className="truncate text-xs text-muted">Desktop</p>
          </div>
        </div>
        <div className="flex items-center gap-1.5">
          <div className="flex h-7 items-center gap-2 rounded-control border border-border px-2 text-xs text-secondary">
            <span
              className={cn("size-1.5 rounded-full", connected ? statusTone(status) : "bg-danger")}
              aria-hidden="true"
            />
            <span>{connected ? (status === "ready" ? "Ready" : "Degraded") : "Unavailable"}</span>
          </div>
          <ThemeButton />
        </div>
      </header>

      <div className="grid min-h-0 grid-cols-[52px_224px_minmax(420px,1fr)_288px]">
        <nav aria-label="Primary" className="flex flex-col items-center gap-1 border-r border-border bg-panel py-2">
          <Button variant="ghost" size="icon" aria-label="Runs" title="Runs" aria-current="page">
            <Layers3 aria-hidden="true" className="size-4" />
          </Button>
          <Button variant="ghost" size="icon" aria-label="Operations" title="Operations" disabled>
            <Activity aria-hidden="true" className="size-4" />
          </Button>
        </nav>

        <aside className="min-h-0 border-r border-border bg-panel">
          <div className="flex h-10 items-center border-b border-border px-3">
            <h2 className="text-xs font-semibold uppercase text-secondary">Runs</h2>
          </div>
          <div className="px-3 py-4 text-sm text-muted">No active run</div>
        </aside>

        <main className="min-h-0 overflow-auto bg-background">
          {hasError ? (
            <div className="grid h-full place-items-center p-8">
              <section aria-labelledby="service-unavailable-title" className="max-w-sm text-center">
                <CircleAlert aria-hidden="true" className="mx-auto mb-3 size-6 text-danger" />
                <h2 id="service-unavailable-title" className="text-base font-semibold">
                  Local service unavailable
                </h2>
                <p className="mt-1 text-sm leading-6 text-secondary">
                  The desktop could not read the local service status.
                </p>
                <Button className="mt-4" onClick={() => void refresh()}>
                  <RefreshCw aria-hidden="true" className="size-3.5" />
                  Retry
                </Button>
              </section>
            </div>
          ) : (
            <div className="grid h-full place-items-center p-8">
              <div className="text-center">
                <div className="mx-auto mb-3 grid size-9 place-items-center rounded-control border border-border bg-panel">
                  <TerminalSquare aria-hidden="true" className="size-4 text-secondary" />
                </div>
                <h2 className="text-sm font-medium">No active run</h2>
              </div>
            </div>
          )}
        </main>

        <aside aria-labelledby="readiness-title" className="min-h-0 overflow-auto border-l border-border bg-panel">
          <div className="flex h-10 items-center border-b border-border px-3">
            <h2 id="readiness-title" className="text-xs font-semibold uppercase text-secondary">
              Readiness
            </h2>
          </div>
          <div className="divide-y divide-border">
            {(readiness.data?.workflows ?? []).map((workflow) => (
              <div key={workflow.workflow} className="px-3 py-2.5">
                <div className="flex items-center gap-2">
                  <span className={cn("size-1.5 rounded-full", statusTone(workflow.status))} aria-hidden="true" />
                  <span className="min-w-0 truncate text-xs font-medium">{workflow.workflow}</span>
                </div>
                <p className="mt-1 text-xs capitalize text-muted">{workflow.status}</p>
              </div>
            ))}
            {(readiness.data?.dependencies ?? []).map((dependency) => (
              <div key={dependency.dependency} className="px-3 py-2.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="min-w-0 truncate text-xs text-secondary">{dependency.dependency}</span>
                  <span className={cn("size-1.5 shrink-0 rounded-full", statusTone(dependency.status))} aria-hidden="true" />
                </div>
              </div>
            ))}
          </div>
        </aside>
      </div>

      <footer className="flex items-center justify-between border-t border-border bg-panel px-3 text-[11px] text-muted">
        <div className="flex items-center gap-2">
          <span className={cn("size-1.5 rounded-full", connected ? "bg-success" : "bg-danger")} aria-hidden="true" />
          <span>{connected ? `Service ${health.data.lifecycle}` : "Service disconnected"}</span>
        </div>
        <span>{version.data === undefined ? "Version unavailable" : `v${version.data.version}`}</span>
      </footer>
    </div>
  );
}
