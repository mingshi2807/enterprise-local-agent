import { invoke } from "@tauri-apps/api/core";
import type { z } from "zod";

import {
  approvalPreviewSchema,
  buildInfoSchema,
  healthSchema,
  readinessSchema,
  runViewSchema,
  serviceEventsSchema,
  sessionSchema,
  waitingPageSchema,
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
  startLocalWriteRun: (sessionId: string, startRequestId: string, input: string) =>
    invokeAndValidate("conversation_start_localwrite_run", runViewSchema, {
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
  listWaiting: () => invokeAndValidate("approval_list_waiting", waitingPageSchema),
  approvalPreview: (sessionId: string, runId: string, waitId: string) =>
    invokeAndValidate("approval_get_preview", approvalPreviewSchema, { sessionId, runId, waitId }),
  submitApprovalDecision: async (
    sessionId: string,
    runId: string,
    waitId: string,
    expectedRowVersion: number,
    decision: "approve" | "deny",
  ) => {
    await invoke("approval_submit_decision", { sessionId, runId, waitId, expectedRowVersion, decision });
  },
  resumeWaiting: async (sessionId: string, runId: string, waitId: string) => {
    await invoke("approval_resume_run", { sessionId, runId, waitId });
  },
  abortWaiting: async (sessionId: string, runId: string, waitId: string, expectedRowVersion: number) => {
    await invoke("approval_abort_waiting", { sessionId, runId, waitId, expectedRowVersion });
  },
} as const;
