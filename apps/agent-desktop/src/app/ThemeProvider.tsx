import { useLayoutEffect, useMemo, useState, useSyncExternalStore, type PropsWithChildren } from "react";

import {
  ThemeContext,
  type ResolvedTheme,
  type ThemeContextValue,
  type ThemePreference,
} from "@/app/ThemeContext";

const STORAGE_KEY = "ela-desktop-theme";
const DARK_QUERY = "(prefers-color-scheme: dark)";

function storedPreference(): ThemePreference {
  const value = window.localStorage.getItem(STORAGE_KEY);
  return value === "light" || value === "dark" ? value : "system";
}

function subscribeToSystemTheme(onChange: () => void) {
  const media = window.matchMedia(DARK_QUERY);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}

function systemThemeIsDark() {
  return window.matchMedia(DARK_QUERY).matches;
}

export function ThemeProvider({ children }: PropsWithChildren) {
  const [preference, setPreferenceState] = useState<ThemePreference>(storedPreference);
  const systemDark = useSyncExternalStore(subscribeToSystemTheme, systemThemeIsDark, () => false);
  const resolved: ResolvedTheme = preference === "system" ? (systemDark ? "dark" : "light") : preference;

  useLayoutEffect(() => {
    document.documentElement.classList.toggle("dark", resolved === "dark");
    document.documentElement.dataset.theme = resolved;
    document.documentElement.style.colorScheme = resolved;
  }, [resolved]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      preference,
      resolved,
      setPreference(next) {
        window.localStorage.setItem(STORAGE_KEY, next);
        setPreferenceState(next);
      },
    }),
    [preference, resolved],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
