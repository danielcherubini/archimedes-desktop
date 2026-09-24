import { memo, useEffect, useMemo, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import { createHighlighter, type Highlighter } from "shiki";
import type { Message } from "../store/sessions";
import ToolCallCard from "./ToolCallCard";
import DiffBlock from "./DiffBlock";
import { Reasoning, ReasoningTrigger, ReasoningContent } from "./Reasoning";

// One shared highlighter for the whole app.
let highlighterPromise: Promise<Highlighter> | null = null;
function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ["github-dark"],
      langs: [
        "bash",
        "rust",
        "typescript",
        "javascript",
        "json",
        "python",
        "toml",
        "yaml",
      ],
    });
  }
  return highlighterPromise;
}

function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHighlighter().then((highlighter) => {
      if (cancelled) return;
      try {
        setHtml(
          highlighter.codeToHtml(code, {
            lang: lang ?? "plaintext",
            theme: "github-dark",
          }),
        );
      } catch {
        setHtml(null);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [code, lang]);

  if (!html) {
    return (
      <pre className="overflow-x-auto rounded-lg bg-surface p-3 font-mono text-sm">
        <code>{code}</code>
      </pre>
    );
  }
  return (
    <div
      className="overflow-x-auto rounded-lg bg-surface p-3 font-mono text-sm"
      // Shiki output is produced locally from the user's own agent output.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/**
 * The ZCode markdown scale (the `@tailwindcss/typography` `prose*` classes
 * are NOT used — the component mapping below replaces them): h1 `text-ui-xl`,
 * h2 `text-ui-lg`, h3–h6 `text-ui-base` + weight ramp, inline code
 * `font-mono text-ui-sm` on the inline-code token, code blocks
 * `rounded-lg bg-surface` at 14px mono, links `text-ui-base` in the
 * icon-blue token, blocks on the 4px rhythm (`my-2`/`space-y-2`).
 */
function AgentMarkdown({ text }: { text: string }) {
  const components = useMemo(
    () => ({
      h1: ({ children }: { children?: ReactNode }) => (
        <h1 className="my-2 text-ui-xl">{children}</h1>
      ),
      h2: ({ children }: { children?: ReactNode }) => (
        <h2 className="my-2 text-ui-lg">{children}</h2>
      ),
      h3: ({ children }: { children?: ReactNode }) => (
        <h3 className="my-2 text-ui-base font-semibold">{children}</h3>
      ),
      h4: ({ children }: { children?: ReactNode }) => (
        <h4 className="my-2 text-ui-base font-semibold">{children}</h4>
      ),
      h5: ({ children }: { children?: ReactNode }) => (
        <h5 className="my-2 text-ui-base font-medium">{children}</h5>
      ),
      h6: ({ children }: { children?: ReactNode }) => (
        <h6 className="my-2 text-ui-base font-normal">{children}</h6>
      ),
      p: ({ children }: { children?: ReactNode }) => (
        <p className="my-2 text-ui-base">{children}</p>
      ),
      ul: ({ children }: { children?: ReactNode }) => (
        <ul className="my-2 list-disc space-y-2 pl-5 text-ui-base">
          {children}
        </ul>
      ),
      ol: ({ children }: { children?: ReactNode }) => (
        <ol className="my-2 list-decimal space-y-2 pl-5 text-ui-base">
          {children}
        </ol>
      ),
      table: ({ children }: { children?: ReactNode }) => (
        <table className="my-2 text-ui-base">{children}</table>
      ),
      a: ({
        children,
        href,
      }: {
        children?: ReactNode;
        href?: string;
      }) => (
        <a href={href} className="text-ui-base text-icon-blue underline">
          {children}
        </a>
      ),
      code({ className, children }: { className?: string; children?: ReactNode }) {
        const codeText = String(children ?? "").replace(/\n$/, "");
        const lang = /language-(\w+)/.exec(className ?? "")?.[1];
        if (lang || codeText.includes("\n")) {
          return <CodeBlock code={codeText} lang={lang} />;
        }
        return (
          <code className="rounded-sm bg-markdown-inline-code px-1 font-mono text-ui-sm">
            {children}
          </code>
        );
      },
    }),
    [],
  );

  return (
    <div className="break-words text-ui-base">
      <ReactMarkdown components={components}>{text}</ReactMarkdown>
    </div>
  );
}

// Memoized: during a live turn EVERY thought/text chunk replaces only the
// trailing message object in the reducer — every other bubble receives the
// SAME reference. Without this, each chunk re-rendered the whole transcript
// (a full ReactMarkdown re-parse per agent-text bubble), which made the app
// render real slow. Only the changed bubble re-renders now.
export default memo(
  function MessageBubble({ message, isStreaming = false }: { message: Message; isStreaming?: boolean }) {
    switch (message.kind) {
      case "user":
        // The design reference: a plain row — no bubble, no avatar.
        // Images render as a read-only thumbnail grid (the transcript is
        // history — NO remove buttons). `data:` URLs are safe here: the
        // transcript is local.
        return (
          <div className="whitespace-pre-wrap text-ui-base text-foreground">
            {message.text}
            {message.images && message.images.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-2">
                {message.images.map((img, i) => (
                  <img
                    key={i}
                    src={`data:${img.mimeType};base64,${img.data}`}
                    alt={img.name}
                    title={img.name}
                    className="max-h-48 max-w-64 rounded-lg border border-input-border object-contain"
                  />
                ))}
              </div>
            )}
          </div>
        );

      case "agent-text":
        return (
          <div className="text-ui-base">
            <AgentMarkdown text={message.text} />
          </div>
        );

      case "agent-thought":
        return (
          <Reasoning
            isStreaming={isStreaming}
            autoCollapseKey={isStreaming ? null : "complete"}
          >
            <ReasoningTrigger streamingText={message.text} />
            <ReasoningContent>{message.text}</ReasoningContent>
          </Reasoning>
        );

      case "tool-call":
        return (
          <div className="w-full">
            <ToolCallCard
              title={message.title}
              status={message.status}
              diff={message.diff}
            />
          </div>
        );

      case "diff":
        return (
          <div className="w-full">
            <DiffBlock path={message.path} patch={message.patch} />
          </div>
        );
    }
  },
);
