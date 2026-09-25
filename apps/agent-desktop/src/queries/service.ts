import { queryOptions, useQueries } from "@tanstack/react-query";

import { localService } from "@/bridge/service";

export const serviceQueryKeys = {
  all: ["local-service"] as const,
  desktopBuildInfo: () => [...serviceQueryKeys.all, "desktop-build-info"] as const,
  health: () => [...serviceQueryKeys.all, "health"] as const,
  readiness: () => [...serviceQueryKeys.all, "readiness"] as const,
  version: () => [...serviceQueryKeys.all, "version"] as const,
};

export function useServiceState() {
  const [desktopBuildInfo, health, readiness, version] = useQueries({
    queries: [
      queryOptions({
        queryKey: serviceQueryKeys.desktopBuildInfo(),
        queryFn: localService.desktopBuildInfo,
        staleTime: Number.POSITIVE_INFINITY,
      }),
      queryOptions({
        queryKey: serviceQueryKeys.health(),
        queryFn: localService.health,
        refetchInterval: 5_000,
      }),
      queryOptions({
        queryKey: serviceQueryKeys.readiness(),
        queryFn: localService.readiness,
        refetchInterval: 15_000,
      }),
      queryOptions({
        queryKey: serviceQueryKeys.version(),
        queryFn: localService.version,
        staleTime: Number.POSITIVE_INFINITY,
      }),
    ],
  });

  return { desktopBuildInfo, health, readiness, version };
}
