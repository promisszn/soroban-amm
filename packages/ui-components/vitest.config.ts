import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // Ensure the suite and the package scripts never touch the network.
    pool: "forks",
    coverage: {
      provider: "v8",
      reporter: ["text", "json-summary", "html"],
      include: [
        "src/lib/**/*.{ts,tsx}",
        "src/RangeSelector.tsx",
        "src/FeeTierComparison.tsx",
        "src/CapitalEfficiencyCalc.tsx",
        "src/RiskIndicator.tsx",
        "src/PositionManager.tsx",
        "src/GovernanceForum.tsx",
      ],
      exclude: ["src/test/**", "src/**/*.d.ts"],
      thresholds: {
        // Enforced rather than merely reported: a drop below these fails the
        // `test:coverage` run.
        lines: 70,
        functions: 60,
        statements: 70,
        branches: 55,
      },
    },
  },
});
