import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { StatusTag, type StatusKind } from "./StatusTag";

const KINDS: StatusKind[] = [
  "success",
  "info",
  "warning",
  "error",
  "provisional",
  "neutral",
];

describe("StatusTag", () => {
  it.each(KINDS)(
    "renders %s with a label, icon, and description (never color-only)",
    (kind) => {
      const { container } = render(<StatusTag kind={kind} />);
      const tag = container.querySelector(".cds--tag");
      expect(tag).not.toBeNull();
      // Icon present (svg), label text present, description available both as
      // tooltip and as assistive text.
      expect(container.querySelector("svg")).not.toBeNull();
      expect(tag?.textContent?.trim().length ?? 0).toBeGreaterThan(0);
      expect(
        container.querySelector("[title]")?.getAttribute("title"),
      ).toBeTruthy();
      expect(
        container.querySelector(".cds--visually-hidden")?.textContent?.trim(),
      ).toBeTruthy();
    },
  );

  it("uses the localized default labels", () => {
    render(<StatusTag kind="error" />);
    expect(screen.getByText("Failed")).toBeInTheDocument();
  });

  it("accepts label/description overrides", () => {
    render(
      <StatusTag
        kind="success"
        label="Cataloged"
        description="Release is in the catalog"
      />,
    );
    expect(screen.getByText("Cataloged")).toBeInTheDocument();
    expect(screen.getByTitle("Release is in the catalog")).toBeInTheDocument();
  });
});
