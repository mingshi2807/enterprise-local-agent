import { Monitor, Moon, Sun, X } from "lucide-react";
import { useEffect, useRef } from "react";

import { useTheme, type ThemePreference } from "@/app/ThemeContext";
import type { BuildInfo, Readiness } from "@/bridge/contracts";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

type ServiceState = "ready" | "degraded" | "unavailable" | "draining";
type DisplayStatus = "Ready" | "Degraded" | "Unavailable";

interface SettingsViewProps {
  state: ServiceState;
  readiness?: Readiness;
  version?: BuildInfo;
  onClose: () => void;
}

const themes: Array<{ value: ThemePreference; label: string; icon: typeof Monitor }> = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

function statusLabel(status: ServiceState | "ready" | "degraded" | "unavailable"): DisplayStatus {
  if (status === "ready") return "Ready";
  if (status === "unavailable") return "Unavailable";
  return "Degraded";
}

function StatusBadge({ status }: { status: DisplayStatus }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 text-xs font-medium",
        status === "Ready" && "text-success",
        status === "Degraded" && "text-warning",
        status === "Unavailable" && "text-danger",
      )}
    >
      <span className={cn("status-dot", `status-dot-${status.toLowerCase()}`)} aria-hidden="true" />
      {status}
    </span>
  );
}

function SettingRow({ label, detail, children }: { label: string; detail?: string; children: React.ReactNode }) {
  return (
    <div className="grid min-h-11 grid-cols-[minmax(0,1fr)_minmax(0,55%)] items-center gap-4 border-b border-border py-2.5 last:border-b-0">
      <div className="min-w-0">
        <div className="text-[13px] font-medium">{label}</div>
        {detail !== undefined && <div className="mt-0.5 truncate text-xs text-muted">{detail}</div>}
      </div>
      <div className="min-w-0 whitespace-normal text-right text-xs text-secondary">{children}</div>
    </div>
  );
}

function dependencyStatus(readiness: Readiness | undefined, name: string): DisplayStatus {
  const dependency = readiness?.dependencies.find((item) => item.dependency === name);
  return dependency === undefined ? "Unavailable" : statusLabel(dependency.status);
}

function workflowName(workflow: string) {
  if (workflow === "enterprise-engineering-readonly-v1") return "Engineering ReadOnly";
  if (workflow === "enterprise-engineering-localwrite-v1") return "Engineering LocalWrite";
  return workflow;
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1_000) return `${milliseconds}ms`;
  return `${Math.round(milliseconds / 1_000)}s`;
}

export function SettingsView({ state, readiness, version, onClose }: SettingsViewProps) {
  const { preference, setPreference } = useTheme();
  const closeButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    closeButton.current?.focus();
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);
  const principal = readiness?.runtime.principal;
  const localWrite = readiness?.workflows.find(
    (workflow) => workflow.workflow === "enterprise-engineering-localwrite-v1",
  );
  const knowledgeDependencies = readiness?.dependencies.filter(
    ({ dependency }) => dependency.startsWith("knowledge:"),
  ) ?? [];
  const knowledgeLabels = knowledgeDependencies.map(({ dependency }) =>
    dependency === "knowledge:ocpp" ? "OCPP" : dependency === "knowledge:standards" ? "Standards" : "Configured",
  );
  const mcpDependencies = readiness?.dependencies.filter(({ dependency }) => dependency.startsWith("mcp:")) ?? [];

  return (
    <div className="h-full overflow-y-auto" aria-labelledby="settings-heading">
      <div className="mx-auto w-full max-w-3xl px-6 py-5 sm:px-8">
        <div className="mb-7 flex items-center justify-between">
          <div>
            <h2 id="settings-heading" className="text-base font-semibold">Settings</h2>
            <p className="mt-1 text-xs text-muted">Appearance and trusted runtime status</p>
          </div>
          <Button ref={closeButton} type="button" variant="ghost" size="icon" aria-label="Close settings" onClick={onClose}>
            <X aria-hidden="true" className="size-4" />
          </Button>
        </div>

        <section aria-labelledby="appearance-heading" className="border-b border-border pb-7">
          <h3 id="appearance-heading" className="inspector-heading mb-3">Appearance</h3>
          <div className="inline-flex rounded-control border border-border p-0.5" role="radiogroup" aria-label="Theme">
            {themes.map(({ value, label, icon: Icon }) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={preference === value}
                className={cn(
                  "flex h-8 items-center gap-2 rounded-control px-3 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-focus",
                  preference === value ? "bg-selection text-foreground" : "text-muted hover:text-foreground",
                )}
                onClick={() => setPreference(value)}
              >
                <Icon aria-hidden="true" className="size-3.5" />
                {label}
              </button>
            ))}
          </div>
        </section>

        <section aria-labelledby="runtime-heading" className="border-b border-border py-7">
          <h3 id="runtime-heading" className="inspector-heading mb-2">Runtime status</h3>
          <SettingRow label="Agent service" detail="Trusted local runtime">
            <StatusBadge status={statusLabel(state)} />
          </SettingRow>
          <SettingRow label="Model provider" detail="OpenAI-compatible deployment">
            <StatusBadge status={dependencyStatus(readiness, "model")} />
          </SettingRow>
          <SettingRow
            label="Enterprise knowledge"
            detail={knowledgeLabels.length > 0 ? knowledgeLabels.join(" · ") : "Configured retrieval route"}
          >
            <StatusBadge status={dependencyStatus(readiness, "knowledge")} />
          </SettingRow>
          <SettingRow label="LocalWrite" detail="Approval and contained workspace write">
            <StatusBadge
              status={
                localWrite?.enabled === true ? statusLabel(localWrite.status) : "Unavailable"
              }
            />
          </SettingRow>
          <SettingRow label="Containment" detail="Linux LocalWrite isolation">
            <StatusBadge status={dependencyStatus(readiness, "containment")} />
          </SettingRow>
          <SettingRow label="Current identity" detail={principal?.kind.replace("_", " ") ?? "Not available"}>
            {principal === undefined ? "Unavailable" : (
              <div>
                <div className="max-w-72 truncate font-mono">{principal.principal_id}</div>
                <div className="mt-0.5 capitalize text-muted">{principal.roles.join(" · ")}</div>
              </div>
            )}
          </SettingRow>
          <SettingRow label="Application version" detail="Desktop and service contract">
            {version === undefined ? "Unavailable" : `${version.application} ${version.version}`}
          </SettingRow>
        </section>

        <section aria-labelledby="workflows-heading" className="border-b border-border py-7">
          <h3 id="workflows-heading" className="inspector-heading mb-2">Enabled workflows</h3>
          {readiness?.workflows.length ? readiness.workflows.map((workflow) => (
            <SettingRow key={workflow.workflow} label={workflowName(workflow.workflow)}>
              <StatusBadge status={workflow.enabled ? statusLabel(workflow.status) : "Unavailable"} />
            </SettingRow>
          )) : <p className="py-3 text-xs text-muted">Workflow status is unavailable.</p>}
        </section>

        <details className="group py-7">
          <summary className="cursor-pointer list-none text-[13px] font-medium outline-none focus-visible:ring-2 focus-visible:ring-focus">
            Advanced operations
            <span className="ml-2 text-xs font-normal text-muted">Read-only metadata</span>
          </summary>
          <div className="mt-4 border-t border-border">
            {readiness?.runtime.workflow_budgets.map((budget) => (
              <SettingRow key={budget.workflow_id} label={`${workflowName(budget.workflow_id)} limits`}>
                <span className="block">
                  {budget.max_model_calls} model · {budget.max_tool_calls} tool · {budget.max_approval_requests} approval
                </span>
                <span className="mt-0.5 block text-muted">
                  {budget.max_graph_steps} graph · {budget.max_iterations} loop · {formatDuration(budget.max_elapsed_millis)}
                </span>
              </SettingRow>
            ))}
            {readiness !== undefined && (
              <SettingRow label="Service limits">
                {readiness.runtime.max_active_runs} active runs · {readiness.runtime.max_run_input_bytes} input bytes
              </SettingRow>
            )}
            {mcpDependencies.map((dependency) => (
              <SettingRow key={dependency.dependency} label="MCP integration" detail={dependency.dependency.slice(4)}>
                <StatusBadge status={statusLabel(dependency.status)} />
              </SettingRow>
            ))}
            {readiness?.reconciliation.access === "authorized" && (
              <SettingRow label="Reconciliation" detail="Operator-visible metadata only">
                {readiness.reconciliation.count === 0
                  ? "No runs require attention"
                  : readiness.reconciliation.count === 1 && !readiness.reconciliation.truncated
                    ? "1 run requires attention"
                    : `${readiness.reconciliation.count}${readiness.reconciliation.truncated ? "+" : ""} runs require attention`}
              </SettingRow>
            )}
            {readiness?.reconciliation.access === "not_authorized" && (
              <SettingRow label="Reconciliation">Operator access required</SettingRow>
            )}
            {readiness?.reconciliation.access === "unavailable" && (
              <SettingRow label="Reconciliation">Status unavailable</SettingRow>
            )}
            {version !== undefined && (
              <SettingRow label="Contract versions" detail="Configuration · store · event · checkpoint">
                {version.config_schema_version} · {version.store_schema_version} · {version.event_schema_version} · {version.checkpoint_schema_version}
              </SettingRow>
            )}
          </div>
        </details>
      </div>
    </div>
  );
}
