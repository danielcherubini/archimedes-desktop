import { memo, useState, useEffect } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SelectGroup,
  SelectLabel,
} from "../components/ui/select";
import { SessionConfigOption } from "../lib/tauri";

interface SessionConfigSelectProps {
  option: SessionConfigOption;
  /**
   * Stable callback taking the option id — ONE useCallback in the consumer
   * serves both selects. A per-option closure (`onSet(option)`) minted per
   * render would defeat the memo below and re-render ~600 mounted (but
   * invisible — Radix keeps closed-select items in a detached
   * DocumentFragment) SelectItems on EVERY composer keystroke.
   */
  onSet: (optionId: string, value: string) => Promise<void>;
}

function SessionConfigSelect({ option, onSet }: SessionConfigSelectProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (error) {
      const timer = setTimeout(() => setError(null), 5000);
      return () => clearTimeout(timer);
    }
  }, [error]);

  const value = typeof option.currentValue === "string" ? option.currentValue : "";

  const handleValueChange = async (val: string) => {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await onSet(option.id, val);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="relative">
      <Select value={value} onValueChange={handleValueChange}>
        <SelectTrigger
          variant="ghost"
          size="sm"
          className="max-w-48"
          disabled={pending}
          aria-label={option.name}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {option.options?.map((opt) => {
            if ("options" in opt) {
              return (
                <SelectGroup key={opt.name}>
                  <SelectLabel>{opt.name}</SelectLabel>
                  {opt.options.map((subOpt) => (
                    <SelectItem key={subOpt.value} value={subOpt.value}>
                      {subOpt.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              );
            }
            return (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.name}
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>
      {error && (
        <span className="text-ui-sm text-destructive absolute left-0 top-full mt-1 z-10 rounded bg-background px-1" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/**
 * MEMO IS THE FIX (perf regression): the composer's `draft` state re-renders
 * `ChatStream` on every keystroke; without this memo (and a stable `onSet`),
 * every keystroke re-rendered the FULL model catalog mounted inside the
 * selects (~600 items × ~8 fibers ≈ 140ms/keystroke — typing lagged a full
 * minute behind). With stable `option`/`onSet` references the subtree is
 * skipped entirely; a new `option` object (config applied) still re-renders.
 */
export default memo(SessionConfigSelect);
