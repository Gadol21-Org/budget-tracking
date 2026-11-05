// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { Combo } from "../components/Combo";

const ITEMS = [
  { value: "1", label: "מזון" },
  { value: "2", label: "תחבורה" },
  { value: "3", label: "מסעדות" },
];

function Host({ disabled = false }: { disabled?: boolean }) {
  const [value, setValue] = useState("");
  return <Combo items={ITEMS} value={value} onChange={setValue} placeholder="חיפוש…" disabled={disabled} />;
}

afterEach(cleanup);

describe("Combo", () => {
  it("opens its list on focus and narrows it as you type", () => {
    render(<Host />);
    const input = screen.getByPlaceholderText("חיפוש…");
    fireEvent.focus(input);
    expect(screen.getAllByRole("option")).toHaveLength(3);
    fireEvent.change(input, { target: { value: "מ" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["מזון", "מסעדות"]);
  });

  it("shows the picked label in the field after a pick, and closes", () => {
    render(<Host />);
    const input = screen.getByPlaceholderText("חיפוש…") as HTMLInputElement;
    fireEvent.focus(input);
    fireEvent.mouseDown(screen.getByRole("option", { name: "תחבורה" }));
    expect(input.value).toBe("תחבורה");
    expect(input.getAttribute("aria-expanded")).toBe("false");
  });

  it("puts the picked label back when what was typed matches nothing and the list is closed", () => {
    render(<Host />);
    const input = screen.getByPlaceholderText("חיפוש…") as HTMLInputElement;
    fireEvent.focus(input);
    fireEvent.mouseDown(screen.getByRole("option", { name: "מזון" }));
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "zzz" } });
    expect(screen.getByText("אין תוצאות")).toBeTruthy();
    fireEvent.click(document.body);
    expect(input.value).toBe("מזון");
  });

  it("takes the first match on Enter", () => {
    render(<Host />);
    const input = screen.getByPlaceholderText("חיפוש…") as HTMLInputElement;
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "תח" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe("תחבורה");
  });

  it("never shows its list while disabled", () => {
    const { rerender } = render(<Host />);
    fireEvent.focus(screen.getByPlaceholderText("חיפוש…"));
    expect(screen.getAllByRole("option")).toHaveLength(3);
    rerender(<Host disabled />);
    expect(screen.getByPlaceholderText("חיפוש…").getAttribute("aria-expanded")).toBe("false");
  });
});
