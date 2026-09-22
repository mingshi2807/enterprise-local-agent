import { useCallback, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type {
  ApplicationCitation,
  ApprovalDecision,
  ApprovalPreview,
  RunView,
  ServiceEvent,
  WaitingApproval,
} from "@/bridge/contracts";
import { localService } from "@/bridge/service";
import { activityFor, mergeEventPage, type ActivityLabel } from "@/features/runActivity";

export type ConversationActivity = ActivityLabel;

export type ConversationErrorKind =
  | "service_unavailable"
  | "model_unavailable"
  | "knowledge_unavailable"
  | "malformed_model_result"
  | "cancelled"
  | "result_unavailable"
  | "manual_reconciliation"
  | "run_failed";

export interface ConversationMessage {
  id: string;
  role: "user" | "assistant" | "error";
  content: string;
  citations?: ApplicationCitation[];
  errorKind?: ConversationErrorKind;
}

export interface ActiveRun {
  runId: string;
  startRequestId: string;
  startedAtMillis: number;
  cursor: number | null;
  events: ServiceEvent[];
  activity: ConversationActivity;
}

export interface Conversation {
  sessionId: string;
  title: string;
  messages: ConversationMessage[];
  activeRun: ActiveRun | null;
  lastPrompt: string;
  lastRun: RunView | null;
  lastRunEvents: ServiceEvent[];
  lastRunStartedAtMillis: number | null;
  approval: WaitingApproval | null;
}

export type WorkflowMode = "readonly" | "localwrite";

export interface ApprovalUiState {
  item: WaitingApproval;
  preview: ApprovalPreview | null;
  loadingPreview: boolean;
  busy: boolean;
  error: "unauthorized" | "stale" | "unavailable" | null;
}

const conversationsKey = ["conversations"] as const;

function titleFor(prompt: string) {
  const firstLine = prompt.split(/\r?\n/, 1)[0]?.trim() ?? "New conversation";
  return firstLine.length > 52 ? `${firstLine.slice(0, 49)}…` : firstLine;
}

function errorMessage(kind: ConversationErrorKind) {
  switch (kind) {
    case "service_unavailable":
      return "The local agent service is unavailable. Your task was not completed.";
    case "model_unavailable":
      return "The configured model is unavailable or timed out.";
    case "knowledge_unavailable":
      return "Enterprise knowledge retrieval is unavailable or timed out.";
    case "malformed_model_result":
      return "The model returned a result that did not match the required answer format.";
    case "cancelled":
      return "This run was cancelled.";
    case "result_unavailable":
      return "This run completed, but its answer is no longer available after restart.";
    case "manual_reconciliation":
      return "This run requires operator reconciliation and cannot be resumed from the desktop.";
    case "run_failed":
      return "The agent could not complete this task.";
  }
}

function classifyFailure(status: RunView, events: ServiceEvent[]): ConversationErrorKind {
  if (status.outcome === "cancelled") return "cancelled";
  if (events.some((event) => event.category === "knowledge" && event.phase === "failed")) {
    return "knowledge_unavailable";
  }
  if (events.some((event) => event.category === "model" && event.phase === "failed")) {
    return "model_unavailable";
  }
  if (events.some((event) => event.category === "model" && event.phase === "completed")) {
    return "malformed_model_result";
  }
  return "run_failed";
}

function replaceConversation(
  conversations: Conversation[],
  sessionId: string,
  update: (conversation: Conversation) => Conversation,
) {
  return conversations.map((conversation) =>
    conversation.sessionId === sessionId ? update(conversation) : conversation,
  );
}

export function useConversationController(
  selectedSessionId: string | null,
  onSelectSession: (sessionId: string) => void,
  serviceAvailable: boolean,
) {
  const queryClient = useQueryClient();
  const { data: conversations = [] } = useQuery({
    queryKey: conversationsKey,
    queryFn: async (): Promise<Conversation[]> => [],
    initialData: [],
    staleTime: Infinity,
  });
  const selected = useMemo(
    () => conversations.find(({ sessionId }) => sessionId === selectedSessionId) ?? null,
    [conversations, selectedSessionId],
  );

  const setConversations = useCallback((update: (current: Conversation[]) => Conversation[]) => {
    queryClient.setQueryData<Conversation[]>(conversationsKey, (current = []) => update(current));
  }, [queryClient]);

  const sendMutation = useMutation({
    mutationFn: async ({ prompt, mode }: { prompt: string; mode: WorkflowMode }) => {
      const existing = selectedSessionId === null
        ? null
        : queryClient
            .getQueryData<Conversation[]>(conversationsKey)
            ?.find(({ sessionId }) => sessionId === selectedSessionId) ?? null;
      if (existing?.activeRun !== null && existing !== null) throw new Error("active_run");

      const sessionId = existing?.sessionId ?? (await localService.createSession()).session_id;
      const userMessage: ConversationMessage = {
        id: crypto.randomUUID(),
        role: "user",
        content: prompt,
      };
      const startRequestId = crypto.randomUUID();

      setConversations((current) => {
        if (existing === null) {
          return [
            {
              sessionId,
              title: titleFor(prompt),
              messages: [userMessage],
              activeRun: null,
              lastPrompt: prompt,
              lastRun: null,
              lastRunEvents: [],
              lastRunStartedAtMillis: null,
              approval: null,
            },
            ...current,
          ];
        }
        return replaceConversation(current, sessionId, (conversation) => ({
          ...conversation,
          messages: [...conversation.messages, userMessage],
          lastPrompt: prompt,
        }));
      });
      onSelectSession(sessionId);

      try {
        const run = mode === "localwrite"
          ? await localService.startLocalWriteRun(sessionId, startRequestId, prompt)
          : await localService.startReadonlyRun(sessionId, startRequestId, prompt);
        setConversations((current) =>
          replaceConversation(current, sessionId, (conversation) => ({
            ...conversation,
            activeRun: {
              runId: run.run_id,
              startRequestId,
              startedAtMillis: Date.now(),
              cursor: null,
              events: [],
              activity: "Starting…",
            },
            lastRun: run,
          })),
        );
        return sessionId;
      } catch {
        setConversations((current) =>
          replaceConversation(current, sessionId, (conversation) => ({
            ...conversation,
            messages: [
              ...conversation.messages,
              {
                id: crypto.randomUUID(),
                role: "error",
                content: errorMessage("service_unavailable"),
                errorKind: "service_unavailable",
              },
            ],
          })),
        );
        return sessionId;
      }
    },
  });

  const waiting = useQuery({
    queryKey: ["waiting-approvals"],
    queryFn: () => localService.listWaiting(),
    enabled: serviceAvailable,
    refetchInterval: 750,
    retry: false,
  });

  useEffect(() => {
    if (waiting.data === undefined) return;
    const items = waiting.data.items;
    setConversations((current) => {
      let next = current.map((conversation) => {
        const item = items.find(({ run_id }) => run_id === conversation.activeRun?.runId)
          ?? items.find(({ run_id }) => run_id === conversation.lastRun?.run_id)
          ?? null;
        if (item === null || conversation.approval?.row_version === item.row_version) return conversation;
        const activity: ConversationActivity = item.state === "waiting" ? "Waiting for decision…" : "Approval required";
        return {
          ...conversation,
          approval: item,
          activeRun: conversation.activeRun === null
            ? conversation.activeRun
            : { ...conversation.activeRun, activity },
        };
      });
      for (const item of items) {
        if (next.some(({ sessionId }) => sessionId === item.session_id)) continue;
        next = [{
          sessionId: item.session_id,
          title: "Durable LocalWrite approval",
          messages: [],
          activeRun: {
            runId: item.run_id,
            startRequestId: "restored",
            startedAtMillis: Date.now(),
            cursor: null,
            events: [],
            activity: item.state === "waiting" ? "Waiting for decision…" : "Approval required",
          },
          lastPrompt: "",
          lastRun: null,
          lastRunEvents: [],
          lastRunStartedAtMillis: null,
          approval: item,
        }, ...next];
      }
      return next;
    });
    if (selectedSessionId === null && items[0] !== undefined) onSelectSession(items[0].session_id);
  }, [onSelectSession, selectedSessionId, setConversations, waiting.data]);

  const active = selected?.activeRun ?? null;
  const activeRunId = active?.runId ?? null;
  const poll = useQuery({
    queryKey: ["conversation-poll", selectedSessionId, active?.runId],
    enabled: selectedSessionId !== null && active !== null,
    queryFn: async () => {
      if (selectedSessionId === null || active === null) throw new Error("inactive_run");
      const events = await localService.readEvents(selectedSessionId, active.runId, active.cursor);
      const status = await localService.runStatus(selectedSessionId, active.runId);
      return { events, status };
    },
    refetchInterval: 400,
    retry: 2,
  });

  useEffect(() => {
    if (selectedSessionId === null || activeRunId === null || poll.data === undefined) return;
    const { events: page, status } = poll.data;
    setConversations((current) =>
      replaceConversation(current, selectedSessionId, (conversation) => {
        if (conversation.activeRun?.runId !== activeRunId) return conversation;
        const merged = mergeEventPage(
          conversation.activeRun.events,
          conversation.activeRun.cursor,
          page,
          activeRunId,
        );
        if (!merged.valid) {
          return {
            ...conversation,
            activeRun: { ...conversation.activeRun, activity: "Reconnecting…" },
          };
        }
        const events = merged.events;
        const cursor = merged.cursor;
        const freshPage = events.slice(conversation.activeRun.events.length);
        const terminal = status.disposition === "completed"
          || status.disposition === "failed"
          || status.disposition === "manual_reconciliation_required";
        const caughtUp = status.last_sequence === null || (cursor !== null && cursor >= status.last_sequence);
        if (!terminal || !caughtUp) {
          return {
            ...conversation,
            activeRun: {
              ...conversation.activeRun,
              cursor,
              events,
              activity: status.disposition === "waiting"
                ? "Waiting for decision…"
                : terminal
                ? "Finishing…"
                : activityFor(freshPage, conversation.activeRun.activity),
            },
            lastRun: status,
          };
        }

        if (status.result?.kind === "final_answer") {
          return {
            ...conversation,
            activeRun: null,
            lastRun: status,
            lastRunEvents: events,
            lastRunStartedAtMillis: conversation.activeRun.startedAtMillis,
            messages: [
              ...conversation.messages,
              {
                id: crypto.randomUUID(),
                role: "assistant",
                content: status.result.answer,
                citations: status.result.citations,
              },
            ],
          };
        }

        if (status.result?.kind === "local_write_completed") {
          return {
            ...conversation,
            activeRun: null,
            approval: null,
            lastRun: status,
            lastRunEvents: events,
            lastRunStartedAtMillis: conversation.activeRun.startedAtMillis,
            messages: [...conversation.messages, {
              id: crypto.randomUUID(), role: "assistant", content: "The workspace file was written successfully.",
            }],
          };
        }

        if (status.result?.kind === "approval_denied") {
          return {
            ...conversation,
            activeRun: null,
            approval: null,
            lastRun: status,
            lastRunEvents: events,
            lastRunStartedAtMillis: conversation.activeRun.startedAtMillis,
            messages: [...conversation.messages, {
              id: crypto.randomUUID(), role: "error", content: "The LocalWrite request was denied.", errorKind: "run_failed",
            }],
          };
        }

        const errorKind = status.disposition === "manual_reconciliation_required"
          ? "manual_reconciliation"
          : status.disposition === "completed"
          ? "result_unavailable"
          : classifyFailure(status, events);
        return {
          ...conversation,
          activeRun: null,
          lastRun: status,
          lastRunEvents: events,
          lastRunStartedAtMillis: conversation.activeRun.startedAtMillis,
          messages: [
            ...conversation.messages,
            {
              id: crypto.randomUUID(),
              role: "error",
              content: errorMessage(errorKind),
              errorKind,
            },
          ],
        };
      }),
    );
  }, [activeRunId, poll.data, selectedSessionId, setConversations]);

  useEffect(() => {
    if (selectedSessionId === null || activeRunId === null || !poll.isError) return;
    setConversations((current) =>
      replaceConversation(current, selectedSessionId, (conversation) =>
        conversation.activeRun?.runId === activeRunId
          ? {
              ...conversation,
              activeRun: { ...conversation.activeRun, activity: "Reconnecting…" },
            }
          : conversation,
      ),
    );
  }, [activeRunId, poll.isError, selectedSessionId, setConversations]);

  const cancelMutation = useMutation({
    mutationFn: async () => {
      if (selectedSessionId === null || active === null) return;
      setConversations((current) =>
        replaceConversation(current, selectedSessionId, (conversation) =>
          conversation.activeRun === null
            ? conversation
            : {
                ...conversation,
                activeRun: { ...conversation.activeRun, activity: "Stopping…" },
              },
        ),
      );
      await localService.cancelRun(selectedSessionId, active.runId);
    },
  });

  const approval = selected?.approval ?? null;
  const [approvalActionError, setApprovalActionError] = useState<ApprovalUiState["error"]>(null);
  const preview = useQuery({
    queryKey: ["approval-preview", approval?.session_id, approval?.run_id, approval?.wait_id, approval?.row_version],
    queryFn: () => {
      if (approval === null) throw new Error("missing_approval");
      return localService.approvalPreview(approval.session_id, approval.run_id, approval.wait_id);
    },
    enabled: approval !== null && approval.state !== "executing",
    retry: false,
  });

  const approvalMutation = useMutation({
    mutationFn: async (operation: { kind: "decision"; decision: ApprovalDecision } | { kind: "resume" } | { kind: "abort" }) => {
      setApprovalActionError(null);
      if (approval === null) throw new Error("missing_approval");
      if (operation.kind === "decision") {
        await localService.submitApprovalDecision(
          approval.session_id,
          approval.run_id,
          approval.wait_id,
          approval.row_version,
          operation.decision,
        );
      } else if (operation.kind === "resume") {
        await localService.resumeWaiting(approval.session_id, approval.run_id, approval.wait_id);
        setConversations((current) => replaceConversation(current, approval.session_id, (conversation) => ({
          ...conversation,
          approval: null,
          activeRun: conversation.activeRun === null ? null : { ...conversation.activeRun, activity: "Resuming…" },
        })));
      } else {
        await localService.abortWaiting(
          approval.session_id,
          approval.run_id,
          approval.wait_id,
          approval.row_version,
        );
        setConversations((current) => replaceConversation(current, approval.session_id, (conversation) => ({
          ...conversation,
          approval: null,
          activeRun: conversation.activeRun === null ? null : { ...conversation.activeRun, activity: "Finishing…" },
        })));
      }
    },
    onSuccess: (_result, operation) => {
      if (approval === null || operation.kind !== "decision") return;
      setConversations((current) => replaceConversation(current, approval.session_id, (conversation) => ({
        ...conversation,
        approval: conversation.approval === null ? null : {
          ...conversation.approval,
          row_version: conversation.approval.row_version + 1,
          state: operation.decision === "approve" ? "approved" : "denied",
        },
        activeRun: conversation.activeRun === null ? null : {
          ...conversation.activeRun,
          activity: "Approval required",
        },
      })));
    },
    onError: (error) => {
      if (typeof error === "object" && error !== null && "code" in error) {
        if (error.code === "unauthorized") {
          setApprovalActionError("unauthorized");
          return;
        }
        if (error.code === "state_conflict") {
          setApprovalActionError("stale");
          return;
        }
      }
      setApprovalActionError("unavailable");
    },
    onSettled: () => {
      void waiting.refetch();
    },
  });

  function approvalError(): ApprovalUiState["error"] {
    if (approvalActionError !== null) return approvalActionError;
    if (!preview.isError) return null;
    const error = preview.error;
    if (typeof error === "object" && error !== null && "code" in error) {
      if (error.code === "unauthorized") return "unauthorized";
      if (error.code === "state_conflict") return "stale";
    }
    return "unavailable";
  }

  return {
    conversations,
    selected,
    send: (prompt: string, mode: WorkflowMode = "readonly") => sendMutation.mutateAsync({ prompt, mode }),
    cancel: () => cancelMutation.mutateAsync(),
    retry: selected === null || selected.lastPrompt === "" ? null : () => sendMutation.mutateAsync({ prompt: selected.lastPrompt, mode: "readonly" }),
    sending: sendMutation.isPending,
    cancelling: cancelMutation.isPending,
    approval: approval === null ? null : {
      item: approval,
      preview: preview.data ?? null,
      loadingPreview: preview.isPending,
      busy: approvalMutation.isPending,
      error: approvalError(),
    },
    decideApproval: (decision: ApprovalDecision) => approvalMutation.mutateAsync({ kind: "decision", decision }),
    resumeApproval: () => approvalMutation.mutateAsync({ kind: "resume" }),
    abortApproval: () => approvalMutation.mutateAsync({ kind: "abort" }),
  };
}
