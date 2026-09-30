import { memo, useEffect, useMemo, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { createHighlighter, type Highlighter } from "shiki";
import { ChevronRightIcon, WandSparklesIcon } from "lucide-react";
import type { Message } from "../store/sessions";
import { splitSkillBlocks, type SkillBlock } from "../lib/skills";
import ToolCallCard from "./ToolCallCard";
import SubagentDelegatingCard from "./SubagentDelegatingCard";
import DiffBlock from "./DiffBlock";
import { Reasoning, ReasoningTrigger, ReasoningContent } from "./Reasoning";

/**
 * One `<skill>` block injected into a user message: a collapsible card
 * (COLLAPSED by default — ZCode's `ToolLayout` defaults `isOpen` to
 * `false`), styled after the `ToolCallCard`/`FileSummaryCard` conventions:
 * icon + muted label + mono name + a chevron that rotates on open.
 */
function SkillBlockCard({ block }: { block: SkillBlock }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="rounded-lg border border-input-border bg-input px-3 py-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 text-left"
      >
        <WandSparklesIcon className="size-4 shrink-0 text-foreground-subtle" />
        <span className="shrink-0 whitespace-nowrap font-medium text-foreground-subtlest">
          Skill
        </span>
        <span className="min-w-0 truncate font-mono text-ui-sm text-foreground-subtle">
          {block.name}
        </span>
        <ChevronRightIcon
          aria-hidden
          className={`ml-auto size-4 shrink-0 text-foreground-subtlest transition-transform ${
            open ? "rotate-90" : "rotate-0"
          }`}
        />
      </button>
      {open && (
        <div className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-ui-sm text-foreground-subtle">
          {block.body}
        </div>
      )}
    </div>
  );
}

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
      <pre className="my-4 overflow-x-auto rounded-lg border border-border bg-card p-3 font-mono text-sm">
        <code>{code}</code>
      </pre>
    );
  }
  return (
    <div
      className="my-4 overflow-x-auto rounded-lg border border-border bg-card p-3 font-mono text-sm"
      // Shiki output is produced locally from the user's own agent output.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/**
 * The ZCode markdown scale, ported from `zcode/packages/ui/src/components/ai-elements/`
 * (`@tailwindcss/typography` `prose*` classes are NOT used — the component
 * mapping below replaces them):
 * - root: `text-ui-base` on `leading-[1.75] tracking-wide`, first/last block
 *   margins trimmed;
 * - plain `p`: unstyled (preflight zero — paragraphs ride the 1.75 leading);
 * - `strong` `font-medium` (not the default bold);
 * - headings `mt-6 mb-4` + the size/weight ramp (h1 `text-ui-xl` … h6
 *   `text-ui-base` `font-normal`);
 * - lists `my-3` + `space-y-1.5` + `marker:text-foreground-subtlest` (ol is
 *   `list-inside` so multi-digit markers never clip), `li` `pl-1` with direct
 *   `p`s inlined;
 * - blockquote `my-4` + `border-l-2 pl-3 text-foreground-subtle`;
 * - code: inline `font-mono text-ui-sm` on the inline-code token (50%), blocks
 *   `my-4` on a `border border-border bg-card` panel at 14px mono;
 * - links `text-ui-base font-medium` in the icon-blue token, dotted underline
 *   shown on hover;
 * - tables (the ZCode `markdown-table` scale): a `my-3` frame that is
 *   `overflow-x-auto rounded-xl border border-border` (wide tables scroll
 *   instead of breaking the bubble), the table itself `w-max min-w-full`,
 *   cells `px-3 py-2` with a `border-b` row rule, `th` unbolded +
 *   `text-foreground-subtlest`, `td` `align-top`, rows highlight on hover and
 *   the last row drops its rule.
 */
function AgentMarkdown({ text }: { text: string }) {
  const components = useMemo(
    () => ({
      h1: ({ children }: { children?: ReactNode }) => (
        <h1 className="mt-6 mb-4 text-ui-xl font-semibold">{children}</h1>
      ),
      h2: ({ children }: { children?: ReactNode }) => (
        <h2 className="mt-6 mb-4 text-ui-lg font-semibold">{children}</h2>
      ),
      h3: ({ children }: { children?: ReactNode }) => (
        <h3 className="mt-6 mb-4 text-ui-base font-semibold">{children}</h3>
      ),
      h4: ({ children }: { children?: ReactNode }) => (
        <h4 className="mt-6 mb-4 text-ui-base font-semibold">{children}</h4>
      ),
      h5: ({ children }: { children?: ReactNode }) => (
        <h5 className="mt-6 mb-4 text-ui-base font-medium">{children}</h5>
      ),
      h6: ({ children }: { children?: ReactNode }) => (
        <h6 className="mt-6 mb-4 text-ui-base font-normal">{children}</h6>
      ),
      strong: ({ children }: { children?: ReactNode }) => (
        <strong className="font-medium">{children}</strong>
      ),
      ul: ({ children }: { children?: ReactNode }) => (
        <ul className="my-3 list-outside list-disc space-y-1.5 pl-5 marker:text-foreground-subtlest [&_ul]:my-1.5 [&_ol]:my-1.5">
          {children}
        </ul>
      ),
      ol: ({ children }: { children?: ReactNode }) => (
        <ol className="my-3 list-inside list-decimal space-y-1.5 pl-0 marker:text-foreground-subtlest [&_ul]:my-1.5 [&_ol]:my-1.5">
          {children}
        </ol>
      ),
      li: ({ children }: { children?: ReactNode }) => (
        <li className="pl-1 [&>p]:my-0 [&>p]:inline">{children}</li>
      ),
      blockquote: ({ children }: { children?: ReactNode }) => (
        <blockquote className="my-4 border-border border-l-2 pl-3 text-foreground-subtle [&_p]:my-0 [&_p+p]:mt-2">
          {children}
        </blockquote>
      ),
      // The ZCode `markdown-table` scale: a bordered, horizontally
      // scrollable frame around a content-width table (wide tables scroll
      // instead of breaking the bubble); cells get a bottom border, padding,
      // and a min/max width so long content wraps instead of blowing out a
      // column. (ZCode's virtual scrollbar / toolbar / preview dialog are NOT
      // ported — the app's global native-scrollbar styling covers scrolling.)
      table: ({ children }: { children?: ReactNode }) => (
        <div className="my-3 w-full overflow-x-auto rounded-xl border border-border">
          <table className="w-max min-w-full text-ui-base">{children}</table>
        </div>
      ),
      thead: ({ children }: { children?: ReactNode }) => (
        <thead>{children}</thead>
      ),
      tbody: ({ children }: { children?: ReactNode }) => (
        <tbody>{children}</tbody>
      ),
      tr: ({ children }: { children?: ReactNode }) => (
        <tr className="transition-colors last:[&>td]:border-b-0 hover:bg-hover/20">
          {children}
        </tr>
      ),
      th: ({ children }: { children?: ReactNode }) => (
        <th className="border-border border-b px-3 py-2 text-left font-normal text-foreground-subtlest min-w-16 max-w-md whitespace-normal break-words">
          {children}
        </th>
      ),
      td: ({ children }: { children?: ReactNode }) => (
        <td className="border-border border-b px-3 py-2 text-foreground align-top min-w-16 max-w-md whitespace-normal break-words">
          {children}
        </td>
      ),
      a: ({
        children,
        href,
      }: {
        children?: ReactNode;
        href?: string;
      }) => (
        <a
          href={href}
          className="text-ui-base font-medium text-icon-blue no-underline decoration-dotted underline-offset-4 hover:underline"
        >
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
          <code className="rounded-md bg-markdown-inline-code/50 mx-0.5 px-1.5 py-0.5 font-mono text-ui-sm">
            {children}
          </code>
        );
      },
    }),
    [],
  );

  return (
    <div className="break-words text-ui-base leading-[1.75] tracking-wide [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
      {/* `remark-gfm` is NOT included by default in react-markdown v10 —
       * without it GFM tables (and strikethrough / task lists) fall back to
       * raw pipe text, which is what made tables look broken in the chat. */}
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

// Memoized: during a live turn EVERY thought/text chunk replaces only the
// trailing message object in the reducer — every other bubble receives the
// SAME reference. Without this, each chunk re-rendered the whole transcript
// (a full ReactMarkdown re-parse per agent-text bubble), which made the app
// render real slow. Only the changed bubble re-renders now.
export default memo(
  function MessageBubble({
    message,
    isStreaming = false,
    sessionId,
  }: {
    message: Message;
    isStreaming?: boolean;
    /**
     * The ACP session id (optional — the `SubagentTranscript`'s
     * `MessageBubble` does not pass it; a subagent session never has a
     * `subagent` tool call, so the `subagent` branch is never taken there).
     */
    sessionId?: string;
  }) {
    switch (message.kind) {
      case "user": {
        // The design reference: a plain row — no bubble, no avatar.
        // `expandSkillMentions` appends `<skill>` blocks after the user's
        // text; render the blocks as collapsible cards (collapsed by
        // default) instead of raw text — display-only, the persisted/sent
        // text is unchanged. Images render as a read-only thumbnail grid
        // (the transcript is history — NO remove buttons). `data:` URLs are
        // safe here: the transcript is local.
        const { text, blocks } = splitSkillBlocks(message.text);
        return (
          <div className="whitespace-pre-wrap text-ui-base text-foreground">
            {text}
            {blocks.length > 0 && (
              <div className="mt-2 flex flex-col gap-2">
                {blocks.map((block, i) => (
                  <SkillBlockCard key={i} block={block} />
                ))}
              </div>
            )}
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
      }

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
            {message.title === "subagent" ? (
              <SubagentDelegatingCard
                title={message.title}
                status={message.status}
                rawInput={message.rawInput}
                rawOutput={message.rawOutput}
                sessionId={sessionId ?? ""}
              />
            ) : (
              <ToolCallCard
                title={message.title}
                status={message.status}
                diff={message.diff}
                rawInput={message.rawInput}
                rawOutput={message.rawOutput}
              />
            )}
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
