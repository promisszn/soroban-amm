import { describe, it, expect } from "vitest";
import { validatePosition } from "./positionValidation.js";

const valid = {
  amountA: 100,
  amountB: 200,
  priceRange: { lower: 80, upper: 120 },
};

describe("validatePosition", () => {
  it("accepts a fully valid position (no errors)", () => {
    expect(validatePosition(valid)).toEqual([]);
  });

  it("rejects a negative amountA", () => {
    const errs = validatePosition({ ...valid, amountA: -1 });
    expect(errs.some((e) => /Amount A/.test(e))).toBe(true);
  });

  it("rejects a negative amountB", () => {
    const errs = validatePosition({ ...valid, amountB: -50 });
    expect(errs.some((e) => /Amount B/.test(e))).toBe(true);
  });

  it("rejects a zero amount", () => {
    expect(validatePosition({ ...valid, amountA: 0 }).length).toBeGreaterThan(0);
    expect(validatePosition({ ...valid, amountB: 0 }).length).toBeGreaterThan(0);
  });

  it("rejects a lower tick at or above the upper tick", () => {
    expect(validatePosition({ ...valid, priceRange: { lower: 120, upper: 120 } }))
      .toContain("Lower price must be below the upper price.");
    expect(validatePosition({ ...valid, priceRange: { lower: 200, upper: 100 } }))
      .toContain("Lower price must be below the upper price.");
  });

  it("rejects non-finite and non-positive bounds", () => {
    expect(validatePosition({ ...valid, priceRange: { lower: 0, upper: 100 } })
      .some((e) => /lower/i.test(e))).toBe(true);
    expect(validatePosition({ ...valid, priceRange: { lower: 80, upper: NaN } })
      .some((e) => /upper/i.test(e))).toBe(true);
  });

  it("collects multiple errors at once", () => {
    const errs = validatePosition({ amountA: 0, amountB: -1, priceRange: { lower: 120, upper: 100 } });
    expect(errs.length).toBeGreaterThanOrEqual(3);
  });
});
