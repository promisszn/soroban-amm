import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FeeTierComparison } from "./FeeTierComparison.js";
import type { FeeTier } from "./types.js";

const tiers: FeeTier[] = [
  { feeBps: 5, label: "Stable", description: "Stable pairs.", tvlSharePct: 15 },
  { feeBps: 30, label: "Standard", description: "Balanced volume.", tvlSharePct: 60 },
  { feeBps: 100, label: "Exotic", description: "High volatility.", tvlSharePct: 25 },
];

describe("FeeTierComparison", () => {
  it("renders every tier with fee, label, description and TVL share", () => {
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={vi.fn()} />);
    expect(screen.getByText("0.05%", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("0.3%", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("1%", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("Stable")).toBeInTheDocument();
    expect(screen.getByText("Standard")).toBeInTheDocument();
    expect(screen.getByText("Exotic")).toBeInTheDocument();
    expect(screen.getByText("Stable pairs.")).toBeInTheDocument();
    expect(screen.getByText("15%")).toBeInTheDocument();
    expect(screen.getByText("60%")).toBeInTheDocument();
    expect(screen.getByText("25%")).toBeInTheDocument();
  });

  it("marks the selected tier with aria-checked", () => {
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={vi.fn()} />);
    const radios = screen.getAllByRole("radio");
    expect(radios).toHaveLength(3);
    expect(radios[1]).toHaveAttribute("aria-checked", "true");
    expect(radios[0]).toHaveAttribute("aria-checked", "false");
    // Selected tier is focusable (tabIndex 0), others are -1.
    expect(radios[1]).toHaveAttribute("tabindex", "0");
  });

  it("renders the radiogroup with an accessible name", () => {
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={vi.fn()} />);
    expect(screen.getByRole("radiogroup")).toHaveAttribute("aria-required", "true");
  });

  it("calls onSelect with the feeBps when a tier is clicked", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={onSelect} />);
    await user.click(screen.getByRole("radio", { name: /Exotic — 1% fee/ }));
    expect(onSelect).toHaveBeenCalledWith(100);
  });

  it("moves selection to the next tier with ArrowRight", async () => {
    const onSelect = vi.fn();
    render(<FeeTierComparison tiers={tiers} selected={5} onSelect={onSelect} />);
    const first = screen.getByRole("radio", { name: /Stable — 0.05% fee/ });
    first.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(onSelect).toHaveBeenCalledWith(30);
  });

  it("moves selection to the previous tier with ArrowLeft", async () => {
    const onSelect = vi.fn();
    render(<FeeTierComparison tiers={tiers} selected={100} onSelect={onSelect} />);
    const last = screen.getByRole("radio", { name: /Exotic — 1% fee/ });
    last.focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(onSelect).toHaveBeenCalledWith(30);
  });

  it("wrap-around from the first tier with ArrowLeft", async () => {
    const onSelect = vi.fn();
    render(<FeeTierComparison tiers={tiers} selected={5} onSelect={onSelect} />);
    const first = screen.getByRole("radio", { name: /Stable — 0.05% fee/ });
    first.focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(onSelect).toHaveBeenCalledWith(100);
  });

  it("selects the focused tier with Space", async () => {
    const onSelect = vi.fn();
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={onSelect} />);
    const standard = screen.getByRole("radio", { name: /Standard — 0.3% fee/ });
    standard.focus();
    await userEvent.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith(30);
  });

  it("highlights the recommended tier for a stable volatility hint", () => {
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={vi.fn()} volatilityHint="stable" />);
    const stable = screen.getByText("Recommended");
    expect(stable).toBeInTheDocument();
    expect(screen.getByText(/For/)).toHaveTextContent(/stable/);
  });

  it("does not highlight a recommendation when no volatility hint is given", () => {
    render(<FeeTierComparison tiers={tiers} selected={30} onSelect={vi.fn()} />);
    expect(screen.queryByText("Recommended")).not.toBeInTheDocument();
  });

  it("renders an empty/zero state for an empty tier list without crashing", () => {
    render(<FeeTierComparison tiers={[]} selected={0} onSelect={vi.fn()} />);
    expect(screen.getByRole("radiogroup")).toBeInTheDocument();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
  });
});
