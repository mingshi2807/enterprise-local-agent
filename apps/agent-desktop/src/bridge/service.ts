import { invoke } from "@tauri-apps/api/core";
import type { z } from "zod";

import { buildInfoSchema, healthSchema, readinessSchema } from "@/bridge/contracts";

async function invokeAndValidate<T>(command: string, schema: z.ZodType<T>): Promise<T> {
  const response: unknown = await invoke(command);
  return schema.parse(response);
}

export const localService = {
  health: () => invokeAndValidate("service_health", healthSchema),
  readiness: () => invokeAndValidate("service_readiness", readinessSchema),
  version: () => invokeAndValidate("service_version", buildInfoSchema),
} as const;
