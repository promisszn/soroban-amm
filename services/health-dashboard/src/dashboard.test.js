import assert from "node:assert/strict";
import test from "node:test";
import { compareThreshold, formatError, formatMetric, formatTimestamp, healthColor, renderAlertConfigs, renderFiredAlerts } from "./dashboard.js";
import { installFakeDocument } from "./dom-stub.js";

test("formats millions", () => assert.equal(formatMetric(1_250_000), "1.25M"));
test("formats thousands", () => assert.equal(formatMetric(12_500), "12.5K"));
test("formats small metrics", () => assert.equal(formatMetric(12.5), "12.50"));
test("handles non-numeric metrics", () => assert.equal(formatMetric("not-a-number"), "—"));
test("formats timestamps", () => assert.notEqual(formatTimestamp(Date.now()), "—"));
test("compares above thresholds", () => assert.equal(compareThreshold(11, 10), true));
test("compares below thresholds", () => assert.equal(compareThreshold(9, 10, "below"), true));
test("rejects invalid threshold values", () => assert.equal(compareThreshold("x", 10), false));
test("selects health colors", () => { assert.equal(healthColor(80), "var(--green)"); assert.equal(healthColor(50), "var(--yellow)"); assert.equal(healthColor(10), "var(--red)"); });
test("formats invocation errors", () => assert.equal(formatError(new Error("offline")), "offline"));

test("fired alerts render into their own container and survive renderAlertConfigs running afterward", () => {
  const doc = installFakeDocument(["fired-alert-list", "alert-list"]);
  renderFiredAlerts([{ metric: "tvl", threshold: 100, currentValue: 200, firedAt: Date.now() }]);
  renderAlertConfigs([{ poolId: "pool-a", metric: "tvl", thresholdBps: 100 }], () => {});

  const firedList = doc.getElementById("fired-alert-list");
  const configList = doc.getElementById("alert-list");
  assert.equal(firedList.children.length, 1);
  assert.equal(configList.children.length, 1);
});

test("renderFiredAlerts does not accumulate stale entries across calls", () => {
  const doc = installFakeDocument(["fired-alert-list"]);
  renderFiredAlerts([{ metric: "tvl", threshold: 100, currentValue: 200, firedAt: Date.now() }]);
  renderFiredAlerts([{ metric: "volume24h", threshold: 50, currentValue: 60, firedAt: Date.now() }]);

  const firedList = doc.getElementById("fired-alert-list");
  assert.equal(firedList.children.length, 1);
  assert.ok(firedList.children[0].textContent.includes("volume24h"));
});

test("renderAlertConfigs still renders configured thresholds with working remove buttons", () => {
  const doc = installFakeDocument(["alert-list"]);
  let removed = null;
  renderAlertConfigs(
    [{ poolId: "pool-a", metric: "price_deviation", thresholdBps: 50 }],
    (alert) => { removed = alert; },
  );
  const configList = doc.getElementById("alert-list");
  assert.equal(configList.children.length, 1);
  const button = configList.children[0].children.find((c) => c.tagName === "button" || c.className === "remove");
  assert.ok(button);
  button.listeners.click[0]();
  assert.deepEqual(removed, { poolId: "pool-a", metric: "price_deviation", thresholdBps: 50 });
});
