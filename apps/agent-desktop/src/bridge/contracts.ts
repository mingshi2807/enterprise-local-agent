import { z } from "zod";

const readinessStatus = z.enum(["ready", "degraded", "unavailable"]);
const principalRole = z.enum(["user", "approver", "operator"]);
const uuid = z.string().uuid();
const boundedMetadata = z.string().min(1).max(256);

function boundedUtf8(maxBytes: number) {
  return z.string().refine((value) => new TextEncoder().encode(value).byteLength <= maxBytes);
}

export const healthSchema = z
  .object({
    version: z.literal(1),
    lifecycle: z.enum(["serving", "draining"]),
  })
  .strict();

export const readinessSchema = z
  .object({
    version: z.literal(1),
    overall: readinessStatus,
    dependencies: z
      .array(
        z
          .object({
            dependency: z.string().min(1).max(128),
            status: readinessStatus,
            code: z.string().min(1).max(128),
            checked_unix_seconds: z.number().int().nonnegative(),
          })
          .strict(),
      )
      .max(64),
    workflows: z
      .array(
        z
          .object({
            workflow: z.string().min(1).max(128),
            enabled: z.boolean(),
            required: z.boolean(),
            status: readinessStatus,
          })
          .strict(),
      )
      .max(32),
    runtime: z
      .object({
        principal: z
          .object({
            principal_id: z.string().min(1).max(256),
            kind: z.enum(["human", "service", "local_process"]),
            roles: z.array(principalRole).min(1).max(3),
          })
          .strict(),
        max_active_runs: z.number().int().positive(),
        max_run_input_bytes: z.number().int().positive(),
        max_read_page_items: z.number().int().positive().max(64),
        workflow_budgets: z
          .array(
            z
              .object({
                workflow_id: z.string().min(1).max(128),
                max_model_calls: z.number().int().nonnegative(),
                max_tool_calls: z.number().int().nonnegative(),
                max_iterations: z.number().int().nonnegative(),
                max_approval_requests: z.number().int().nonnegative(),
                max_graph_steps: z.number().int().nonnegative().max(64),
                max_elapsed_millis: z.number().int().positive(),
              })
              .strict(),
          )
          .max(32),
      })
      .strict(),
    reconciliation: z.discriminatedUnion("access", [
      z
        .object({
          access: z.literal("authorized"),
          count: z.number().int().nonnegative().max(64),
          truncated: z.boolean(),
        })
        .strict(),
      z.object({ access: z.literal("not_authorized") }).strict(),
      z.object({ access: z.literal("unavailable") }).strict(),
    ]),
  })
  .strict();

export const buildInfoSchema = z
  .object({
    application: z.string().min(1).max(128),
    version: z.string().min(1).max(64),
    config_schema_version: z.number().int().nonnegative(),
    store_schema_version: z.number().int().nonnegative(),
    event_schema_version: z.number().int().nonnegative(),
    checkpoint_schema_version: z.number().int().nonnegative(),
  })
  .strict();

export const desktopBuildInfoSchema = z
  .object({
    application: z.literal("enterprise-local-agent-desktop"),
    version: z.string().regex(/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/).max(64),
    git_revision: z.string().regex(/^[0-9a-fA-F]{40}$/).nullable(),
    build_profile: z.enum(["debug", "release", "unknown"]),
    service_api_version: z.literal(1),
    service_event_version: z.literal(2),
  })
  .strict();

export const compatibilitySchema = z
  .object({
    state: z.enum(["compatible", "legacy_unsupported", "incompatible"]),
    service_generation: uuid.nullable(),
  })
  .strict()
  .refine(
    ({ state, service_generation }) =>
      (state === "compatible" && service_generation !== null) ||
      (state !== "compatible" && service_generation === null),
  );

export const sessionSchema = z.object({ session_id: uuid }).strict();

const applicationCitationSchema = z
  .object({
    evidence_id: z.string().min(1).max(1024),
    backend: z.string().min(1).max(1024),
    source_id: z.string().min(1).max(1024),
    reference_id: z.string().min(1).max(1024),
    provenance: z.string().min(1).max(1024).nullable(),
  })
  .strict();

const finalAnswerResultSchema = z.object({
    kind: z.literal("final_answer"),
    answer: boundedUtf8(8 * 1024).pipe(z.string().min(1)),
    citations: z.array(applicationCitationSchema).max(8),
  })
  .strict();

const applicationResultSchema = z.discriminatedUnion("kind", [
  finalAnswerResultSchema,
  z.object({ kind: z.literal("local_write_completed"), tool_call_id: uuid }).strict(),
  z.object({ kind: z.literal("approval_denied") }).strict(),
]);

export const workflowIdSchema = z.enum([
  "enterprise-engineering-readonly-v1",
  "enterprise-engineering-localwrite-v1",
]);

export const runViewSchema = z
  .object({
    session_id: uuid,
    run_id: uuid,
    disposition: z.enum([
      "starting",
      "running",
      "waiting",
      "resumable",
      "completed",
      "failed",
      "manual_reconciliation_required",
    ]),
    last_sequence: z.number().int().nonnegative().nullable(),
    outcome: z.enum(["completed", "cancelled", "budget_exceeded", "failed"]).nullable(),
    workflow_id: workflowIdSchema.nullable(),
    result: applicationResultSchema.nullable(),
    duration_millis: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const runHistoryItemSchema = z.object({
  run_id: uuid,
  disposition: runViewSchema.shape.disposition,
  last_sequence: z.number().int().nonnegative().nullable(),
  outcome: runViewSchema.shape.outcome,
  workflow_id: workflowIdSchema.nullable(),
  started_at_unix_millis: z.number().int().nonnegative().nullable(),
  result_available: z.boolean(),
}).strict();

export const runHistoryPageSchema = z.object({
  items: z.array(runHistoryItemSchema).max(64),
  next_run_id: uuid.nullable(),
}).strict();

export const conversationSummarySchema = z.object({
  session_id: uuid,
  title: boundedMetadata,
  last_activity_unix_millis: z.number().int().nonnegative().nullable(),
  latest_run: runHistoryItemSchema.nullable(),
}).strict();

export const conversationPageSchema = z.object({
  items: z.array(conversationSummarySchema).max(64),
  next_session_id: uuid.nullable(),
}).strict();

export const serviceEventSchema = z
  .object({
    version: z.literal(2),
    sequence: z.number().int().nonnegative(),
    run_id: uuid,
    category: z.enum([
      "run",
      "model",
      "action",
      "tool",
      "approval",
      "containment",
      "knowledge",
      "loop",
      "graph",
      "audit",
    ]),
    phase: z.enum([
      "started",
      "completed",
      "failed",
      "proposed",
      "validated",
      "rejected",
      "bound",
      "denied",
      "granted",
      "prepared",
      "decision_recorded",
      "degraded",
      "progress",
      "suspended",
      "resumed",
      "finished",
    ]),
    correlation_id: boundedMetadata.nullable(),
    workflow_id: workflowIdSchema.nullable(),
    knowledge_backends: z.array(boundedMetadata).max(8),
    graph_node_id: boundedMetadata.nullable(),
    budget_usage: z.number().int().nonnegative().nullable(),
    budget_limit: z.number().int().nonnegative().nullable(),
  })
  .strict()
  .refine(
    ({ budget_usage, budget_limit }) =>
      (budget_usage === null && budget_limit === null) ||
      (budget_usage !== null && budget_limit !== null && budget_usage <= budget_limit),
  );

export const serviceEventsSchema = z.array(serviceEventSchema).max(64);

const waitingApprovalSchema = z.object({
  session_id: uuid,
  run_id: uuid,
  wait_id: uuid,
  row_version: z.number().int().nonnegative(),
  state: z.enum(["waiting", "approved", "denied", "executing"]),
}).strict();

export const waitingPageSchema = z.object({
  items: z.array(waitingApprovalSchema).max(64),
  next_run_id: uuid.nullable(),
  next_wait_id: uuid.nullable(),
}).strict().refine(({ next_run_id, next_wait_id }) => (next_run_id === null) === (next_wait_id === null));

export const approvalPreviewSchema = z.object({
  wait_id: uuid,
  row_version: z.number().int().nonnegative(),
  operation: z.literal("Write workspace file"),
  target: boundedUtf8(1024).pipe(z.string().min(1)),
  content_bytes: z.number().int().nonnegative().max(4 * 1024),
}).strict();

export type Health = z.infer<typeof healthSchema>;
export type Readiness = z.infer<typeof readinessSchema>;
export type BuildInfo = z.infer<typeof buildInfoSchema>;
export type DesktopBuildInfo = z.infer<typeof desktopBuildInfoSchema>;
export type DesktopCompatibility = z.infer<typeof compatibilitySchema>;
export type ServiceConnectionState =
  | "ready"
  | "degraded"
  | "unavailable"
  | "draining"
  | "upgrade_required"
  | "incompatible";
export type Session = z.infer<typeof sessionSchema>;
export type RunView = z.infer<typeof runViewSchema>;
export type RunHistoryItem = z.infer<typeof runHistoryItemSchema>;
export type RunHistoryPage = z.infer<typeof runHistoryPageSchema>;
export type ConversationSummary = z.infer<typeof conversationSummarySchema>;
export type ConversationPage = z.infer<typeof conversationPageSchema>;
export type ApplicationCitation = z.infer<typeof applicationCitationSchema>;
export type ServiceEvent = z.infer<typeof serviceEventSchema>;
export type WorkflowId = z.infer<typeof workflowIdSchema>;
export type WaitingApproval = z.infer<typeof waitingApprovalSchema>;
export type WaitingPage = z.infer<typeof waitingPageSchema>;
export type ApprovalPreview = z.infer<typeof approvalPreviewSchema>;
export type ApprovalDecision = "approve" | "deny";
