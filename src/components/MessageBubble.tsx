import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { createHighlighter, type Highlighter } from "shiki";
import {
  CheckIcon,
  ChevronRightIcon,
  CopyIcon,
  WandSparklesIcon,
} from "lucide-react";
import type { Message } from "../store/sessions";
import { useSettings } from "../store/settings";
import type { AppPalette } from "../lib/theme";
import { splitSkillBlocks, type SkillBlock } from "../lib/skills";
import { fileIconFor } from "../lib/fileIcons";
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
    <div className="rounded-lg bg-input px-3 py-2">
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

/**
 * One shared highlighter for the whole app.
 *
 * BOTH syntax themes are loaded in this ONE call (ADR 0027): switching
 * palette then only re-runs `codeToHtml` against themes the singleton already
 * holds — no second highlighter, no re-instantiation, no flash of unhighlighted
 * code. (`createHighlighter` is the expensive part — grammars + themes — so a
 * per-palette instance would pay it twice for a purely cosmetic axis.)
 *
 * `tsx` / `jsx` are in the `langs` list because they are the alias grammars a
 * coding agent emits by far the most. Shiki THROWS on a lang outside this list
 * (`Language `tsx` not found`), the `codeToHtml` call site below catches it,
 * and the catch renders the plain fallback — so before these two entries
 * existed every tsx / jsx fence was silently UNHIGHLIGHTED with no error
 * anywhere. `css` / `html` / `markdown` / `vue` are still absent and still
 * fall back the same way (a known gap, deliberately not widened here).
 */
let highlighterPromise: Promise<Highlighter> | null = null;
function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ["github-dark", "dracula"],
      langs: [
        "bash",
        "rust",
        "typescript",
        "javascript",
        "tsx",
        "jsx",
        "json",
        "python",
        "toml",
        "yaml",
      ],
    });
  }
  return highlighterPromise;
}

/**
 * (ADR 0027) The syntax theme DERIVES from the palette — no separate setting.
 *
 * Deliberately a one-line pure function exported for tests: the rule is then
 * assertable without a DOM, and the render tests below only have to prove the
 * rule is WIRED into `codeToHtml` (via the theme's own paper colour).
 *
 * Anything that is not `"dracula"` — `"zai"`, `null` (settings not loaded /
 * the stored default), `undefined` — answers `github-dark`, which is the
 * behaviour this component had before the palette axis existed. The pairing is
 * coherent in both directions: `dracula` is a DARK syntax theme and only
 * Dracula's dark variant ships here (a scope decision — ADR 0027), so that
 * palette pins the app dark and dark code never sits on a light bubble.
 */
export function shikiThemeFor(
  palette: AppPalette | null | undefined,
): "dracula" | "github-dark" {
  return palette === "dracula" ? "dracula" : "github-dark";
}

/**
 * The ONE body class string, shared by BOTH render branches.
 *
 * It used to be two literals, and they drifted: the fallback `<pre>` carried
 * `p-3` and the highlighted `<div>` did not, so every real fence sat flush
 * against the panel edge while the rare fallback was inset — the opposite of
 * what the doc-comment below promises. Shiki emits no padding (its
 * `<pre class="shiki …">` carries only an inline colour) and no rule in
 * `index.css` pads `pre`, `.shiki` or `code`, so the class was the entire
 * inset. `MessageBubble.codeblock.test.tsx` now asserts the two branches emit
 * the SAME class list, which is only possible while it is one constant.
 *
 * Deliberately NO background class: the highlighted branch must not fight the
 * inline paper colour Shiki writes on its own `<pre>`.
 */
const CODE_BODY_CLASS = "overflow-x-auto p-3 font-mono text-sm";

/**
 * A fenced code block: a bordered panel whose header names the file type
 * (glyph + hue from `fileIconFor`, so a ```tsx fence is the SAME colour as the
 * `.tsx` chip in a tool row) and offers a copy button, above the
 * Shiki-highlighted body. Only the HEADER is a `bg-panel` band: Shiki writes
 * an INLINE `background-color` onto its own `<pre>` (theme paper), which
 * outranks any Tailwind class, so the body is deliberately left unstyled in
 * the background axis — the CSS states what actually renders instead of
 * asserting a class that loses.
 *
 * Why the header is `bg-panel` and not `bg-card`: the descriptor hue it paints
 * is the SAME token a `FileChip` uses, and a `FileChip` rides the transcript
 * column — its row (`TRANSCRIPT_ROW`) carries no background of its own, so the
 * chip shows the column behind it, which is `bg-chat` (`ChatStream.tsx`)
 * = the content slab `#343746` under Dracula. The card is the raised `#424450`,
 * and 7 of the 25 descriptors fall below the 3:1 non-text floor on that ONE
 * surface (`file-ts`/`file-py` 2.50, `file-html` 2.55, `file-sass`/`file-graphql`
 * 2.57, `file-java` 2.62, `file-php` 2.63) while all 25 clear it on the chrome
 * page (worst `file-py` 3.69), on the slab (worst 3.06) and on the panel (worst
 * 4.09). The hues are IDENTITY colours that a palette must not re-hue, so the
 * surface is what moves — which is exactly what the island layout did: it
 * recessed `--color-panel` to `#21222c`, so the header now has MORE headroom
 * for these hues than it had at `#343746` (3.06 → 4.09).
 *
 * Header/body separation is the one thing that got WEAKER, and it is worth
 * stating rather than glossing: the header used to sit ABOVE Shiki's inline
 * `#282A36` paper (`#343746`, a 1.21 step) and now sits BELOW it (`#21222c`, a
 * 1.11 step). The direction inverted and the magnitude shrank by about a tenth.
 * It survives because `border-b` is still there and because 1.11 is still a
 * visible edge between a band and a body, but this is the cost side of the
 * recess, not a free win. `src/lib/fileIconContrast.test.ts` pins both surfaces
 * by TOKEN, so a future move re-measures instead of re-asserting.
 *
 * Ported from ZCode's code block, HEADER ONLY: line numbers, the wrap toggle,
 * Mermaid, the fullscreen viewer and i18n are all out of scope here, and the
 * copy interaction reuses this repo's own `ToolCallCardHeader` pattern
 * (copy → check for 1.5s, timer cleared on unmount) rather than ZCode's.
 *
 * The header renders whether or not the highlight succeeded — the block must
 * never change shape between highlighted and fallback rendering.
 */
function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const [html, setHtml] = useState<string | null>(null);

  // (ADR 0027) Which syntax theme to highlight with. Read straight from the
  // settings store (the same subscription shape `ChatStream` uses for
  // `spinnerStyle`), so a palette change in the settings UI reaches an
  // already-mounted bubble with no reload; `null` (nothing loaded yet, or the
  // stored default) is the zai default.
  const palette = useSettings((s) => s.settings?.palette ?? "zai");

  // Copy → confirm for 1.5s (the `ToolCallCardHeader` pattern verbatim: a
  // ref holds the pending timer so a second click restarts it, and the
  // unmount cleanup clears it — no `setState` on an unmounted bubble).
  const [copied, setCopied] = useState(false);
  const resetRef = useRef<number | null>(null);
  const handleCopy = () => {
    void navigator.clipboard?.writeText(code)?.then(() => {
      setCopied(true);
      if (resetRef.current !== null) window.clearTimeout(resetRef.current);
      resetRef.current = window.setTimeout(() => setCopied(false), 1500);
    });
  };
  useEffect(
    () => () => {
      if (resetRef.current !== null) window.clearTimeout(resetRef.current);
    },
    [],
  );

  // The identity is derived from a synthetic filename so the fence shares the
  // extension table the file chips use. An absent language is `text` (plain
  // text); an UNKOWN one keeps its own label — `bogus` is more truthful than
  // silently claiming the block is plain text, and it still gets the neutral
  // glyph because `bogus` is not in the table.
  const label = lang?.trim() ? lang.trim().toLowerCase() : "text";
  const descriptor = fileIconFor(`x.${label}`);
  const Icon = descriptor.icon;
  // What Shiki is ASKED for, which is NOT the same string as the label above:
  // `text` is this component's own name for "no language" and Shiki has no such
  // alias (it has `plaintext`), and a `TypeScript` / `JSON` fence must be
  // normalised exactly like the label is. Passing the raw `lang` was a bug:
  // Shiki resolves aliases case-sensitively, so a capitalised fence THREW,
  // the `catch` below rendered the unhighlighted fallback, and the header still
  // showed the lowercased label — the fence looked highlighted and was not.
  const shikiLang = lang?.trim() ? label : "plaintext";

  const header = (
    <div
      data-testid="code-block-header"
      className="flex items-center justify-between gap-2 border-b border-border bg-panel px-3 py-1.5"
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <Icon className={`size-3.5 shrink-0 ${descriptor.className}`} />
        <span
          data-testid="code-block-lang"
          className={`${descriptor.className} font-mono text-ui-caption lowercase`}
        >
          {label}
        </span>
      </span>
      <button
        type="button"
        onClick={handleCopy}
        aria-label={copied ? "Copied" : "Copy code"}
        title={copied ? "Copied" : "Copy code"}
        className="shrink-0 text-foreground-subtle hover:text-foreground"
      >
        {copied ? (
          <CheckIcon className="size-3" />
        ) : (
          <CopyIcon className="size-3" />
        )}
      </button>
    </div>
  );

  useEffect(() => {
    let cancelled = false;
    getHighlighter().then((highlighter) => {
      if (cancelled) return;
      try {
        setHtml(
          highlighter.codeToHtml(code, {
            lang: shikiLang,
            theme: shikiThemeFor(palette),
          }),
        );
      } catch {
        setHtml(null);
      }
    });
    return () => {
      cancelled = true;
    };
    // `palette` is a dependency on purpose: it is what makes an ALREADY-mounted
    // fence re-highlight the instant the palette changes (both themes are
    // already loaded, so this is just a re-render of the body).
  }, [code, lang, palette]);

  if (!html) {
    // Unknown / unsupported language (or the highlighter still loading): same
    // panel, same header, plain body.
    return (
      <div className="my-4 overflow-hidden rounded-lg border border-border">
        {header}
        <pre className={CODE_BODY_CLASS}>
          <code>{code}</code>
        </pre>
      </div>
    );
  }
  return (
    <div className="my-4 overflow-hidden rounded-lg border border-border">
      {header}
      <div
        className={CODE_BODY_CLASS}
        // Shiki output is produced locally from the user's own agent output.
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </div>
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
 *   `my-4` on a `border border-border` panel at 14px mono — a file-type header
 *   (`bg-panel`) above the body, which keeps Shiki's own paper;
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
