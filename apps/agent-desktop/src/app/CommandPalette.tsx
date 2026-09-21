import { useEffect, useMemo, useRef, useState } from "react";
import { Search, X, type LucideIcon } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";

import { Button } from "@/components/ui/button";

export interface PaletteCommand {
  id: string;
  label: string;
  shortcut?: string;
  icon: LucideIcon;
  action: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  commands: PaletteCommand[];
  onOpenChange: (open: boolean) => void;
}

export function CommandPalette({ open, commands, onOpenChange }: CommandPaletteProps) {
  const reduceMotion = useReducedMotion();
  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const filtered = useMemo(
    () => commands.filter((command) => command.label.toLowerCase().includes(query.trim().toLowerCase())),
    [commands, query],
  );

  useEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusTimer = window.setTimeout(() => {
      setQuery("");
      setActiveIndex(0);
      inputRef.current?.focus();
    }, 0);
    return () => {
      window.clearTimeout(focusTimer);
      previousFocus?.focus();
    };
  }, [open]);

  const run = (command: PaletteCommand) => {
    command.action();
    onOpenChange(false);
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 flex items-start justify-center bg-overlay px-4 pt-[12vh]"
          initial={reduceMotion ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduceMotion ? 0 : 0.1 }}
          onMouseDown={(event) => {
            if (event.currentTarget === event.target) onOpenChange(false);
          }}
        >
          <motion.div
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="command-palette-title"
            className="w-full max-w-xl overflow-hidden rounded-panel border border-border bg-panel shadow-palette"
            initial={reduceMotion ? false : { y: -8, scale: 0.99 }}
            animate={{ y: 0, scale: 1 }}
            exit={{ y: -5, scale: 0.99 }}
            transition={{ duration: reduceMotion ? 0 : 0.12, ease: "easeOut" }}
            onKeyDown={(event) => {
              if (event.key === "Escape") onOpenChange(false);
              if (event.key === "Tab") {
                const focusable = Array.from(
                  dialogRef.current?.querySelectorAll<HTMLElement>("input, button:not(:disabled)") ?? [],
                );
                if (focusable.length > 0) {
                  const first = focusable[0];
                  const last = focusable[focusable.length - 1];
                  if (event.shiftKey && document.activeElement === first) {
                    event.preventDefault();
                    last.focus();
                  } else if (!event.shiftKey && document.activeElement === last) {
                    event.preventDefault();
                    first.focus();
                  }
                }
              }
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setActiveIndex((index) => Math.min(index + 1, filtered.length - 1));
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setActiveIndex((index) => Math.max(index - 1, 0));
              }
              if (event.key === "Enter" && filtered[activeIndex]) run(filtered[activeIndex]);
            }}
          >
            <h2 id="command-palette-title" className="sr-only">Command palette</h2>
            <div className="flex h-11 items-center gap-2 border-b border-border px-3">
              <Search aria-hidden="true" className="size-4 text-muted" />
              <input
                ref={inputRef}
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActiveIndex(0);
                }}
                aria-label="Search commands"
                placeholder="Type a command"
                className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted"
              />
              <Button variant="ghost" size="icon" aria-label="Close command palette" onClick={() => onOpenChange(false)}>
                <X aria-hidden="true" className="size-4" />
              </Button>
            </div>
            <div className="max-h-72 overflow-auto p-1.5" role="listbox" aria-label="Available commands">
              {filtered.length === 0 ? (
                <p className="px-3 py-6 text-center text-xs text-muted">No matching commands</p>
              ) : (
                filtered.map((item, index) => {
                  const Icon = item.icon;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="option"
                      aria-selected={index === activeIndex}
                      className="flex h-9 w-full items-center gap-2 rounded-control px-2.5 text-left text-sm outline-none hover:bg-selection focus-visible:ring-2 focus-visible:ring-focus aria-selected:bg-selection"
                      onMouseMove={() => setActiveIndex(index)}
                      onClick={() => run(item)}
                    >
                      <Icon aria-hidden="true" className="size-4 text-secondary" />
                      <span>{item.label}</span>
                      {item.shortcut && <kbd className="ml-auto text-[11px] text-muted">{item.shortcut}</kbd>}
                    </button>
                  );
                })
              )}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
