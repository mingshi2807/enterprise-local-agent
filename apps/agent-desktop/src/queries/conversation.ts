import { useCallback, useEffect, useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type { ApplicationCitation, RunView, ServiceEvent } from "@/bridge/contracts";
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
    mutationFn: async (prompt: string) => {
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
        const run = await localService.startReadonlyRun(sessionId, startRequestId, prompt);
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
        const terminal = status.disposition === "completed" || status.disposition === "failed";
        const caughtUp = status.last_sequence === null || (cursor !== null && cursor >= status.last_sequence);
        if (!terminal || !caughtUp) {
          return {
            ...conversation,
            activeRun: {
              ...conversation.activeRun,
              cursor,
              events,
              activity: terminal
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

        const errorKind = status.disposition === "completed"
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

  return {
    conversations,
    selected,
    send: (prompt: string) => sendMutation.mutateAsync(prompt),
    cancel: () => cancelMutation.mutateAsync(),
    retry: selected === null ? null : () => sendMutation.mutateAsync(selected.lastPrompt),
    sending: sendMutation.isPending,
    cancelling: cancelMutation.isPending,
  };
}
