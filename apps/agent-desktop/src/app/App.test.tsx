import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { ThemeProvider } from "@/app/ThemeProvider";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </ThemeProvider>,
  );
}

function mockService({
  lifecycle = "serving",
  overall = "ready",
}: {
  lifecycle?: "serving" | "draining";
  overall?: "ready" | "degraded" | "unavailable";
} = {}) {
  invoke.mockImplementation((command: string) => {
    if (command === "service_health") return Promise.resolve({ version: 1, lifecycle });
    if (command === "service_readiness") {
      return Promise.resolve({
        version: 1,
        overall,
        dependencies: [
          {
            dependency: "knowledge",
            status: overall,
            code: overall,
            checked_unix_seconds: 1,
          },
        ],
        workflows: [
          {
            workflow: "enterprise-engineering-readonly-v1",
            enabled: true,
            required: true,
            status: overall,
          },
        ],
      });
    }
    return Promise.resolve({
      application: "enterprise-local-agent",
      version: "0.1.0",
      git_identity: null,
      deployment_fingerprint: "test",
      config_schema_version: 2,
      store_schema_version: 2,
      event_schema_version: 9,
      checkpoint_schema_version: 4,
    });
  });
}

describe("App", () => {
  afterEach(cleanup);

  beforeEach(() => {
    invoke.mockReset();
    window.localStorage.clear();
    window.innerWidth = 1280;
  });

  it("shows bounded service readiness returned by the typed bridge", async () => {
    mockService();

    renderApp();

    expect(await screen.findByText("enterprise-engineering-readonly-v1")).toBeInTheDocument();
    expect(screen.getByText("Local service ready")).toBeInTheDocument();
    expect(screen.getByText("v0.1.0")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "What are you working on?" })).toBeInTheDocument();
  });

  it("shows a sanitized unavailable state when the bridge fails", async () => {
    invoke.mockRejectedValue(new Error("service_unavailable"));
    renderApp();

    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Local service unavailable" })).toBeInTheDocument(),
    );
    expect(screen.queryByText("service_unavailable")).not.toBeInTheDocument();
  });

  it.each([
    ["degraded", "Local service degraded"],
    ["unavailable", "Local service unavailable"],
  ] as const)("projects the %s readiness state", async (overall, label) => {
    mockService({ overall });
    renderApp();

    if (overall === "unavailable") {
      expect(await screen.findByRole("heading", { name: label })).toBeInTheDocument();
    } else {
      expect(await screen.findByText(label)).toBeInTheDocument();
    }
  });

  it("gives draining lifecycle precedence over readiness", async () => {
    mockService({ lifecycle: "draining" });
    renderApp();

    expect(await screen.findByText("Local service draining")).toBeInTheDocument();
  });

  it("opens and executes the command palette with keyboard controls", async () => {
    mockService();
    renderApp();
    await screen.findByText("Local service ready");

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    expect(dialog).toBeInTheDocument();

    const search = screen.getByRole("textbox", { name: "Search commands" });
    fireEvent.change(search, { target: { value: "inspector" } });
    fireEvent.keyDown(dialog, { key: "Enter" });

    await waitFor(() => expect(screen.queryByRole("heading", { name: "Inspector" })).not.toBeInTheDocument());
  });

  it("supports pane and new-task shortcuts without overriding editor shortcuts", async () => {
    mockService();
    renderApp();
    await screen.findByText("Local service ready");

    fireEvent.click(screen.getByRole("button", { name: /Review charging profile/ }));
    expect(screen.getByRole("heading", { name: "Conversation preview" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "n", ctrlKey: true });
    expect(screen.getByRole("heading", { name: "New task" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "b", ctrlKey: true });
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();

    const composer = screen.getByRole("textbox", { name: "Task composer" });
    composer.focus();
    fireEvent.keyDown(composer, { key: "b", ctrlKey: true });
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "i", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Inspector" })).not.toBeInTheDocument());
  });

  it("prioritizes the task workspace at compact window width", async () => {
    window.innerWidth = 800;
    mockService();
    renderApp();

    await screen.findByText("Local service ready");
    expect(screen.getByRole("button", { name: "Expand conversations" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show inspector" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Inspector" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Task composer" })).toBeInTheDocument();
  });
});
