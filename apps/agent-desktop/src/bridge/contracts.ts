import { z } from "zod";

const readinessStatus = z.enum(["ready", "degraded", "unavailable"]);
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
  })
  .strict();

export const buildInfoSchema = z
  .object({
    application: z.string().min(1).max(128),
    version: z.string().min(1).max(64),
    git_identity: z.string().max(128).nullable(),
    deployment_fingerprint: z.string().min(1).max(256),
    config_schema_version: z.number().int().nonnegative(),
    store_schema_version: z.number().int().nonnegative(),
    event_schema_version: z.number().int().nonnegative(),
    checkpoint_schema_version: z.number().int().nonnegative(),
  })
  .strict();

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

const applicationResultSchema = z
  .object({
    kind: z.literal("final_answer"),
    answer: boundedUtf8(8 * 1024).pipe(z.string().min(1)),
    citations: z.array(applicationCitationSchema).max(8),
  })
  .strict();

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
    last_sequence: z.number().int().positive().nullable(),
    outcome: z.enum(["completed", "cancelled", "budget_exceeded", "failed"]).nullable(),
    workflow_id: z.literal("enterprise-engineering-readonly-v1").nullable(),
    result: applicationResultSchema.nullable(),
    duration_millis: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const serviceEventSchema = z
  .object({
    version: z.literal(2),
    sequence: z.number().int().positive(),
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
    workflow_id: z.literal("enterprise-engineering-readonly-v1").nullable(),
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

export type Health = z.infer<typeof healthSchema>;
export type Readiness = z.infer<typeof readinessSchema>;
export type BuildInfo = z.infer<typeof buildInfoSchema>;
export type Session = z.infer<typeof sessionSchema>;
export type RunView = z.infer<typeof runViewSchema>;
export type ApplicationCitation = z.infer<typeof applicationCitationSchema>;
export type ServiceEvent = z.infer<typeof serviceEventSchema>;
