import { invoke } from "@tauri-apps/api/core";
import type { z } from "zod";

import {
  buildInfoSchema,
  healthSchema,
  readinessSchema,
  runViewSchema,
  serviceEventsSchema,
  sessionSchema,
} from "@/bridge/contracts";

async function invokeAndValidate<T>(command: string, schema: z.ZodType<T>, args?: Record<string, unknown>): Promise<T> {
  const response: unknown = await invoke(command, args);
  return schema.parse(response);
}

export const localService = {
  health: () => invokeAndValidate("service_health", healthSchema),
  readiness: () => invokeAndValidate("service_readiness", readinessSchema),
  version: () => invokeAndValidate("service_version", buildInfoSchema),
  createSession: () => invokeAndValidate("conversation_create_session", sessionSchema),
  startReadonlyRun: (sessionId: string, startRequestId: string, input: string) =>
    invokeAndValidate("conversation_start_readonly_run", runViewSchema, {
      sessionId,
      startRequestId,
      input,
    }),
  runStatus: (sessionId: string, runId: string) =>
    invokeAndValidate("conversation_run_status", runViewSchema, { sessionId, runId }),
  cancelRun: async (sessionId: string, runId: string) => {
    await invoke("conversation_cancel_run", { sessionId, runId });
  },
  readEvents: (sessionId: string, runId: string, afterSequence: number | null) =>
    invokeAndValidate("conversation_read_events", serviceEventsSchema, {
      sessionId,
      runId,
      afterSequence,
    }),
} as const;
