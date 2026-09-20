import { z } from "zod";

const readinessStatus = z.enum(["ready", "degraded", "unavailable"]);

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

export type Health = z.infer<typeof healthSchema>;
export type Readiness = z.infer<typeof readinessSchema>;
export type BuildInfo = z.infer<typeof buildInfoSchema>;
