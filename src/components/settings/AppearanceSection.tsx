import type { ReactElement } from "react";
import { MAX_FONT_PX, MIN_FONT_PX } from "@/lib/settings";
import type { AppSettings } from "@/lib/tauri";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SettingsGroupCard, SettingsRow, closedSelectValue, openSelectItems } from "./primitives";
import { FontSizeInput, SpinnerStylePicker } from "./controls";

/** The Theme picker's options — its CLOSED vocabulary (Radix renders no label
 * for a value outside it), and the single source for both the select's items
 * and the membership check. */
const THEME_OPTIONS = [
  { value: "system", label: "System (follow the OS)" },
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
] as const;
/** The Palette picker's closed vocabulary (ADR 0027). `"zai"` is the SENTINEL
 * the trigger uses for the `null` default — never `""`, which Radix renders as
 * nothing. */
const PALETTE_OPTIONS = [
  { value: "zai", label: "Zai (default)" },
  { value: "dracula", label: "Dracula" },
] as const;

/** The UI font's offered families (the saved value is the QUOTED CSS family
 * name for a web font — `applySettingsFont` splices it into the `--font-sans`
 * stack, where an unquoted multi-word name would not be a valid family. The
 * stack's tail (system + CJK fallbacks) still applies when a web font is
 * unavailable, e.g. offline). The `"default"` sentinel (saved as `null`) is the
 * APP DEFAULT — Noto Sans, which the `index.css` `--font-sans` stack resolves
 * to first, with the system tail as the offline fallback. */
const UI_FONT_OPTIONS = [
  { value: "default", label: "Default (Noto Sans)" },
  { value: "ui-serif, Georgia, serif", label: "Serif" },
  {
    value: "ui-monospace, SFMono-Regular, Menlo, monospace",
    label: "Monospace",
  },
  { value: '"Noto Sans"', label: "Noto Sans" },
  { value: '"Fira Sans"', label: "Fira Sans" },
  { value: '"Martel Sans"', label: "Martel Sans" },
] as const;

/** The Code font's offered families (the `"default"` sentinel and the quoted
 * web-font names follow the UI font's conventions; the value is spliced into
 * the `--font-mono` stack). The default is Fira Code — the `index.css`
 * `--font-mono` stack resolves to it first, with the system + CJK tail as the
 * offline fallback. */
const CODE_FONT_OPTIONS = [
  { value: "default", label: "Default (Fira Code)" },
  { value: '"JetBrains Mono"', label: "JetBrains Mono" },
  { value: '"Fira Code"', label: "Fira Code" },
  { value: '"Cascadia Code"', label: "Cascadia Code" },
  { value: "Menlo", label: "Menlo" },
  { value: "Consolas", label: "Consolas" },
] as const;

/** The Appearance section's props (every piece of state stays in `SettingsPage`). */
interface AppearanceSectionProps {
  settings: AppSettings | null;
  /** The immediate-save pattern. */
  onSave: (patch: Partial<AppSettings>) => void;
}

/** The Appearance section: theme / font size / fonts / thinking spinner. */
export default function AppearanceSection({
  settings,
  onSave,
}: AppearanceSectionProps): ReactElement | null {
  if (settings === null) return null;
  // The font families have an OPEN vocabulary — `applySettingsFont` splices
  // ANY non-null family into the CSS stack — so a stored family the list does
  // not offer (a hand-edited `"Georgia"`) is genuinely applied and is surfaced
  // as its own option rather than coerced to the default, which would lie
  // about the font on screen (the app's pattern for a stored value outside a
  // select's list, as in the default-thinking-level picker).
  const uiFont = openSelectItems(settings.font.uiFamily, UI_FONT_OPTIONS);
  const codeFont = openSelectItems(settings.font.codeFamily, CODE_FONT_OPTIONS);
  const themeValues = THEME_OPTIONS.map((option) => option.value);
  const paletteValues = PALETTE_OPTIONS.map((option) => option.value);
  return (
    <SettingsGroupCard>
      <SettingsRow
        label="Theme"
        control={
          <Select
            // A `theme` outside the vocabulary (a hand-edited file) resolves to
            // the LIGHT mode — `resolveTheme`'s final branch treats anything not
            // `dark` as light — so that is the mode the trigger names, rather
            // than the documented `dark` default (which would describe the
            // default while the window rendered light).
            value={closedSelectValue(settings.theme, themeValues, "light")}
            onValueChange={(value) =>
              onSave({ theme: value as AppSettings["theme"] })
            }
          >
            <SelectTrigger aria-label="Theme" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {THEME_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Palette"
        description="The color scheme. Dracula is dark-only — it overrides the Theme setting (including System)."
        controlLayout="wide"
        control={
          <Select
            // `closedSelectValue`: the backend stores `palette` as an
            // unvalidated string, so any value can arrive (e.g. a hand-edited
            // `"solarized"`). Nothing but `"dracula"` selects a scheme other
            // than Zai, so the Zai label is the honest readout for anything
            // unrecognised — and it keeps the default selectable, instead of
            // blanking the trigger (Radix renders no label for an off-list
            // value). RENDERING ONLY: nothing is re-saved for being unreadable.
            value={closedSelectValue(settings.palette, paletteValues, "zai")}
            onValueChange={(value) =>
              onSave({ palette: value === "zai" ? null : "dracula" })
            }
          >
            {/* The `"zai"` option saves `null` (not `"zai"`) — the same
                default-sentinel pattern as `spinnerStyle` and the font
                families — so a default palette leaves `settings.json`
                minimal, and a pre-feature file (no `palette` key) and an
                explicit Zai choice stay the SAME document (ADR 0027). */}
            <SelectTrigger aria-label="Palette" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PALETTE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Font size"
        description="The UI base size (12–20px)"
        control={
          <FontSizeInput
            value={settings.font.sizePx}
            min={MIN_FONT_PX}
            max={MAX_FONT_PX}
            ariaLabel="Font size"
            onChange={(sizePx) =>
              onSave({ font: { ...settings.font, sizePx } })
            }
          />
        }
      />
      <SettingsRow
        label="UI font"
        control={
          <Select
            value={uiFont.value}
            onValueChange={(value) =>
              onSave({
                font: {
                  ...settings.font,
                  uiFamily: value === "default" ? null : value,
                },
              })
            }
          >
            <SelectTrigger aria-label="UI font" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {uiFont.items.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Code font"
        control={
          <Select
            value={codeFont.value}
            onValueChange={(value) =>
              onSave({
                font: {
                  ...settings.font,
                  codeFamily: value === "default" ? null : value,
                },
              })
            }
          >
            <SelectTrigger aria-label="Code font" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {codeFont.items.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Thinking spinner"
        description="The animation the chat's top working indicator runs while the agent is busy"
        controlLayout="wide"
        control={
          <SpinnerStylePicker
            value={settings.spinnerStyle ?? "typing"}
            onChange={(spinnerStyle) => onSave({ spinnerStyle })}
          />
        }
      />
    </SettingsGroupCard>
  );
}
