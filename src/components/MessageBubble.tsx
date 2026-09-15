import { useEffect, useMemo, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import { createHighlighter, type Highlighter } from "shiki";
import type { Message } from "../store/sessions";
import ToolCallCard from "./ToolCallCard";
import DiffBlock from "./DiffBlock";

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
      <pre className="overflow-x-auto rounded-md bg-neutral-900 p-3 text-sm">
        <code>{code}</code>
      </pre>
    );
  }
  return (
    <div
      className="overflow-x-auto rounded-md text-sm"
      // Shiki output is produced locally from the user's own agent output.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

function AgentMarkdown({ text }: { text: string }) {
  const components = useMemo(
    () => ({
      code({ className, children }: { className?: string; children?: ReactNode }) {
        const codeText = String(children ?? "").replace(/\n$/, "");
        const lang = /language-(\w+)/.exec(className ?? "")?.[1];
        if (lang || codeText.includes("\n")) {
          return <CodeBlock code={codeText} lang={lang} />;
        }
        return (
          <code className="rounded bg-neutral-800 px-1 py-0.5 text-xs">
            {children}
          </code>
        );
      },
    }),
    [],
  );

  return (
    <div className="prose prose-invert prose-sm max-w-none break-words prose-p:my-2 prose-headings:my-3">
      <ReactMarkdown components={components}>{text}</ReactMarkdown>
    </div>
  );
}

export default function MessageBubble({ message }: { message: Message }) {
  switch (message.kind) {
    case "user":
      return (
        <div className="flex justify-end">
          <div className="max-w-[80%] rounded-lg bg-sky-700 px-3 py-2 text-sm whitespace-pre-wrap">
            {message.text}
          </div>
        </div>
      );

    case "agent-text":
      return (
        <div className="flex justify-start">
          <div className="max-w-[90%] rounded-lg border border-neutral-800 bg-neutral-900 px-3 py-2">
            <AgentMarkdown text={message.text} />
          </div>
        </div>
      );

    case "tool-call":
      return (
        <div className="flex justify-start">
          <div className="w-full max-w-[90%]">
            <ToolCallCard
              title={message.title}
              status={message.status}
              diff={message.diff}
            />
          </div>
        </div>
      );

    case "diff":
      return (
        <div className="flex justify-start">
          <div className="w-full max-w-[90%]">
            <DiffBlock path={message.path} patch={message.patch} />
          </div>
        </div>
      );
  }
}
