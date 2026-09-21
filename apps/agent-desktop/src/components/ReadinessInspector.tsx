import { ChevronRight, X } from "lucide-react";

import type { BuildInfo, Readiness } from "@/bridge/contracts";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

type ServiceState = "ready" | "degraded" | "unavailable" | "draining";

interface ReadinessInspectorProps {
  state: ServiceState;
  readiness?: Readiness;
  version?: BuildInfo;
  onClose: () => void;
}

export function ReadinessInspector({ state, readiness, version, onClose }: ReadinessInspectorProps) {
  return (
    <aside aria-labelledby="inspector-title" className="flex h-full w-[300px] flex-col">
      <div className="flex h-10 items-center justify-between border-b border-border px-3">
        <h2 id="inspector-title" className="text-xs font-semibold">Inspector</h2>
        <Button variant="ghost" size="icon" aria-label="Close inspector" onClick={onClose}>
          <X aria-hidden="true" className="size-3.5" />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <section className="border-b border-border px-3 py-3" aria-labelledby="service-section-title">
          <div className="flex items-center justify-between gap-3">
            <h3 id="service-section-title" className="text-[11px] font-semibold uppercase text-muted">Local service</h3>
            <span className="flex items-center gap-1.5 text-[11px] capitalize text-secondary">
              <span className={cn("status-dot", `status-dot-${state}`)} aria-hidden="true" />
              {state}
            </span>
          </div>
          <dl className="mt-3 grid grid-cols-[1fr_auto] gap-x-3 gap-y-2 text-xs">
            <dt className="text-muted">Application</dt>
            <dd className="max-w-40 truncate text-right">{version?.application ?? "Unavailable"}</dd>
            <dt className="text-muted">Version</dt>
            <dd>{version?.version ?? "—"}</dd>
          </dl>
        </section>

        <section className="border-b border-border py-2" aria-labelledby="workflows-section-title">
          <h3 id="workflows-section-title" className="px-3 py-1.5 text-[11px] font-semibold uppercase text-muted">Workflows</h3>
          {(readiness?.workflows ?? []).length === 0 ? (
            <p className="px-3 py-2 text-xs text-muted">No workflow status available</p>
          ) : (
            readiness?.workflows.map((workflow) => (
              <div key={workflow.workflow} className="flex items-center gap-2 px-3 py-2 text-xs">
                <span className={cn("status-dot", `status-dot-${workflow.status}`)} aria-hidden="true" />
                <span className="min-w-0 flex-1 truncate">{workflow.workflow}</span>
                <ChevronRight aria-hidden="true" className="size-3 text-muted" />
              </div>
            ))
          )}
        </section>

        <section className="py-2" aria-labelledby="dependencies-section-title">
          <h3 id="dependencies-section-title" className="px-3 py-1.5 text-[11px] font-semibold uppercase text-muted">Dependencies</h3>
          {(readiness?.dependencies ?? []).length === 0 ? (
            <p className="px-3 py-2 text-xs text-muted">No dependency details available</p>
          ) : (
            readiness?.dependencies.map((dependency) => (
              <div key={dependency.dependency} className="flex items-center gap-2 px-3 py-2 text-xs">
                <span className="min-w-0 flex-1 truncate text-secondary">{dependency.dependency}</span>
                <span className={cn("status-dot", `status-dot-${dependency.status}`)} aria-hidden="true" />
              </div>
            ))
          )}
        </section>
      </div>
    </aside>
  );
}
