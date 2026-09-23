import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it("keeps text-ui-* font-size classes alongside text color classes", () => {
    expect(cn("text-ui-base", "text-foreground")).toContain("text-ui-base");
    expect(cn("text-ui-base", "text-foreground")).toContain("text-foreground");
  });

  it("still merges conflicting classes the normal way", () => {
    const merged = cn("bg-card", "bg-surface");
    expect(merged).toContain("bg-surface");
    expect(merged).not.toContain("bg-card");
  });
});
