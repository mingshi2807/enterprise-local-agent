import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

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

describe("App", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("shows bounded service readiness returned by the typed bridge", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "service_health") return Promise.resolve({ version: 1, lifecycle: "serving" });
      if (command === "service_readiness") {
        return Promise.resolve({
          version: 1,
          overall: "ready",
          dependencies: [],
          workflows: [
            {
              workflow: "enterprise-engineering-readonly-v1",
              enabled: true,
              required: true,
              status: "ready",
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

    renderApp();

    expect(await screen.findByText("enterprise-engineering-readonly-v1")).toBeInTheDocument();
    expect(screen.getByText("Service serving")).toBeInTheDocument();
    expect(screen.getByText("v0.1.0")).toBeInTheDocument();
  });

  it("shows a sanitized unavailable state when the bridge fails", async () => {
    invoke.mockRejectedValue(new Error("service_unavailable"));
    renderApp();

    await waitFor(() => expect(screen.getByText("Local service unavailable")).toBeInTheDocument());
    expect(screen.queryByText("service_unavailable")).not.toBeInTheDocument();
  });
});
