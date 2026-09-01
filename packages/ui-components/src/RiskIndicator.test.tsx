import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RiskIndicator } from "./RiskIndicator.js";
import type { RiskAssessment } from "./types.js";

const assessment: RiskAssessment = {
  level: "high",
  score: 45,
  factors: [
    { name: "Out of range", description: "Price is outside range.", severity: "critical" },
    { name: "High price deviation", description: "Unusual volatility.", severity: "high", value: 600, threshold: 500 },
  ],
};

describe("RiskIndicator", () => {
  it("renders the risk level label and score text", () => {
    render(<RiskIndicator assessment={assessment} />);
    expect(screen.getByText("High Risk")).toBeInTheDocument();
    expect(screen.getByText(/Score: 45\/100/)).toBeInTheDocument();
  });

  it("renders the factor summary count", () => {
    render(<RiskIndicator assessment={assessment} />);
    expect(screen.getByText(/2 risk factors identified/)).toBeInTheDocument();
  });

  it("names singular factor count correctly", () => {
    render(
      <RiskIndicator
        assessment={{ ...assessment, factors: [assessment.factors[0]] }}
      />,
    );
    expect(screen.getByText(/1 risk factor identified/)).toBeInTheDocument();
  });

  it("renders the empty/zero state when there are no factors", () => {
    render(<RiskIndicator assessment={{ level: "low", score: 100, factors: [] }} />);
    expect(screen.getByText(/0 risk factors identified/)).toBeInTheDocument();
    // No expand button when there is nothing to expand.
    expect(screen.queryByRole("button", { name: /details/i })).not.toBeInTheDocument();
  });

  it("is collapsed by default and expands detail factors on toggle", async () => {
    const user = userEvent.setup();
    render(<RiskIndicator assessment={assessment} />);
    expect(screen.queryByText("Unusual volatility.")).not.toBeInTheDocument();
    const toggle = screen.getByRole("button", { name: /Show details/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(screen.getByRole("button", { name: /Hide details/i })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Unusual volatility.")).toBeInTheDocument();
  });

  it("renders expanded by default when defaultExpanded is true", () => {
    render(<RiskIndicator assessment={assessment} defaultExpanded />);
    expect(screen.getByText("Unusual volatility.")).toBeInTheDocument();
  });

  it("renders severity badges with accessible names", () => {
    render(<RiskIndicator assessment={assessment} defaultExpanded />);
    expect(screen.getByLabelText("Severity: Critical Risk")).toBeInTheDocument();
    expect(screen.getByLabelText("Severity: High Risk")).toBeInTheDocument();
  });

  it("exposes an accessible overall risk live region", () => {
    render(<RiskIndicator assessment={assessment} />);
    expect(screen.getByRole("status")).toHaveAttribute(
      "aria-label",
      "Overall risk: High Risk, score 45 out of 100",
    );
  });

  it("honours an aria-label override on the root", () => {
    render(<RiskIndicator assessment={assessment} aria-label="My indicator" />);
    expect(screen.getByLabelText("My indicator")).toBeInTheDocument();
  });

  it("renders a factor bar only when value and threshold are defined", () => {
    render(<RiskIndicator assessment={assessment} defaultExpanded />);
    const list = screen.getByRole("list", { name: "Risk factors" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    // Only one factor (deviation) has value+threshold → one bar rendered.
    const bar = list.querySelector("[aria-hidden='true']");
    expect(bar).toBeInTheDocument();
    expect(list.querySelectorAll("[aria-hidden='true']")).toHaveLength(1);
  });

  it("applies the level border colour to the root", () => {
    const { container } = render(<RiskIndicator assessment={assessment} />);
    expect(container.firstChild).toHaveStyle({ borderColor: "#f0883e" });
  });
});
