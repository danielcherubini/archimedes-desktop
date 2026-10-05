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
import { SettingsGroupCard, SettingsRow } from "./primitives";
import { FontSizeInput, SpinnerStylePicker } from "./controls";

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
  return (
    <SettingsGroupCard>
      <SettingsRow
        label="Theme"
        control={
          <Select
            value={settings.theme}
            onValueChange={(value) =>
              onSave({ theme: value as AppSettings["theme"] })
            }
          >
            <SelectTrigger aria-label="Theme" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">System (follow the OS)</SelectItem>
              <SelectItem value="dark">Dark</SelectItem>
              <SelectItem value="light">Light</SelectItem>
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
            value={settings.font.uiFamily ?? "default"}
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
              {/* The `"default"` sentinel (saved as `null`) is the APP
                  DEFAULT — Noto Sans for the UI font (the `index.css`
                  `--font-sans` stack resolves to it first, with the
                  system tail as the offline fallback). A sentinel value
                  (NOT `""`): Radix's `SelectValue` renders nothing for
                  an empty-string value, so the trigger would go blank
                  when the default is selected. */}
              <SelectItem value="default">Default (Noto Sans)</SelectItem>
              <SelectItem value="ui-serif, Georgia, serif">Serif</SelectItem>
              <SelectItem value="ui-monospace, SFMono-Regular, Menlo, monospace">
                Monospace
              </SelectItem>
              {/* Web fonts (the `index.html` Google Fonts link — the
                  saved value is the QUOTED CSS family name:
                  `applySettingsFont` splices it into the `--font-sans`
                  stack, where an unquoted multi-word name would not be a
                  valid family). The stack's tail (system + CJK
                  fallbacks) still applies when a web font is
                  unavailable, e.g. offline. */}
              <SelectItem value='"Noto Sans"'>Noto Sans</SelectItem>
              <SelectItem value='"Fira Sans"'>Fira Sans</SelectItem>
              <SelectItem value='"Martel Sans"'>Martel Sans</SelectItem>
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Code font"
        control={
          <Select
            value={settings.font.codeFamily ?? "default"}
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
              {/* The `"default"` sentinel (saved as `null`) is the APP
                  DEFAULT — Fira Code for the code font (the `index.css`
                  `--font-mono` stack resolves to it first, with the
                  system + CJK tail as the offline fallback; a sentinel
                  value, NOT `""` — see the UI font's note). The
                  multi-word values are the QUOTED CSS family names (the
                  same convention as the UI font's web-font options —
                  `applySettingsFont` splices the value into the
                  `--font-mono` stack). */}
              <SelectItem value="default">Default (Fira Code)</SelectItem>
              <SelectItem value='"JetBrains Mono"'>JetBrains Mono</SelectItem>
              <SelectItem value='"Fira Code"'>Fira Code</SelectItem>
              <SelectItem value='"Cascadia Code"'>Cascadia Code</SelectItem>
              <SelectItem value="Menlo">Menlo</SelectItem>
              <SelectItem value="Consolas">Consolas</SelectItem>
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
