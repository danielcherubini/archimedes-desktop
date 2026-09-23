import { useState, useEffect } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { SessionConfigOption } from "../lib/tauri";

interface SessionConfigSelectProps {
  option: SessionConfigOption;
  onSet: (value: string) => Promise<void>;
}

export default function SessionConfigSelect({
  option,
  onSet,
}: SessionConfigSelectProps) {
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
      await onSet(val);
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
                <div key={opt.name} className="px-2 py-1 text-ui-xs text-muted-foreground font-medium">
                  {opt.name}
                  {opt.options.map((subOpt) => (
                    <SelectItem key={subOpt.value} value={subOpt.value}>
                      {subOpt.name}
                    </SelectItem>
                  ))}
                </div>
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
