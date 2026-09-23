import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { Button } from "@/components/ui/button";

function nodeText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  if (node !== null && typeof node === "object" && "props" in node) {
    return nodeText((node as { props: { children?: ReactNode } }).props.children);
  }
  return "";
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | null>(null);
  useEffect(() => () => {
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
  }, []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
      resetTimer.current = window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <Button type="button" variant="ghost" size="icon" aria-label={copied ? "Copied" : label} title={copied ? "Copied" : label} onClick={() => void copy()}>
      {copied ? <Check aria-hidden="true" className="size-3.5 text-success" /> : <Copy aria-hidden="true" className="size-3.5" />}
      <span className="sr-only" aria-live="polite">{copied ? `${label} complete` : ""}</span>
    </Button>
  );
}

export const MarkdownAnswer = memo(function MarkdownAnswer({ answer }: { answer: string }) {
  return (
    <div className="answer-wrap">
      <div className="answer-actions"><CopyButton value={answer} label="Copy answer" /></div>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ children }) => <span className="font-medium text-accent">{children}</span>,
          pre: ({ children }) => {
            const code = nodeText(children).replace(/\n$/, "");
            return (
              <div className="code-block">
                <div className="code-actions"><CopyButton value={code} label="Copy code" /></div>
                <pre>{children}</pre>
              </div>
            );
          },
        }}
      >
        {answer}
      </ReactMarkdown>
    </div>
  );
});
