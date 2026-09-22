import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { Button } from "@/components/ui/button";

describe("Button", () => {
  it("renders the default variant with the primary classes", () => {
    render(<Button>Go</Button>);
    const button = screen.getByRole("button", { name: "Go" });
    expect(button.className).toContain("bg-primary");
  });

  it("renders the ghost variant's classes", () => {
    render(<Button variant="ghost">G</Button>);
    const button = screen.getByRole("button", { name: "G" });
    expect(button.className).toContain("hover:bg-hover");
    expect(button.className).toContain("aria-expanded:bg-hover");
  });

  it("renders a square icon button", () => {
    render(<Button size="icon">I</Button>);
    const button = screen.getByRole("button", { name: "I" });
    expect(button.className).toContain("size-7");
  });
});
