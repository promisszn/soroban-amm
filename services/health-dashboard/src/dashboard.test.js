import assert from "node:assert/strict";
import test from "node:test";
import { compareThreshold, createDashboardController, formatError, formatMetric, formatTimestamp, healthColor, renderAlertConfigs, renderFiredAlerts } from "./dashboard.js";
import { installFakeDocument } from "./dom-stub.js";

const QUERY_RESPONSES = {
  Stats: { poolStats: [] },
  Health: { poolHealth: null },
  Alerts: { alertConfigs: [] },
  History: { priceHistory: [] },
  Events: { poolEvents: [] },
};

function installFakeFetch(failingKeys = []) {
  globalThis.fetch = async (_url, options) => {
    const { query } = JSON.parse(options.body);
    const key = Object.keys(QUERY_RESPONSES).find((k) => query.includes(`query ${k}`));
    if (failingKeys.includes(key)) {
      return { ok: false, status: 500, json: async () => ({ errors: [{ message: `${key} failed` }] }) };
    }
    return { ok: true, json: async () => ({ data: QUERY_RESPONSES[key] }) };
  };
}

function setupRefreshTestDom() {
  const doc = installFakeDocument([
    "connection-status", "dashboard-state", "api-url", "pool-id", "btn-retry",
    "m-tvl", "m-vol", "m-fees", "m-swaps", "health-score-text", "last-updated",
    "fired-alert-list", "alert-list",
  ]);
  doc.getElementById("btn-retry").hidden = true;
  return doc;
}

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

test("btn-retry stays hidden after a fully successful refresh", async () => {
  const doc = setupRefreshTestDom();
  installFakeFetch([]);
  const controller = createDashboardController({ apiUrl: "http://api.test", poolId: "pool-a" });
  await controller.refresh();
  assert.equal(doc.getElementById("btn-retry").hidden, true);
});

test("btn-retry is shown after a partial failure", async () => {
  const doc = setupRefreshTestDom();
  installFakeFetch(["Alerts"]);
  const controller = createDashboardController({ apiUrl: "http://api.test", poolId: "pool-a" });
  await controller.refresh();
  assert.equal(doc.getElementById("btn-retry").hidden, false);
});

test("btn-retry is shown after every query fails", async () => {
  const doc = setupRefreshTestDom();
  installFakeFetch(Object.keys(QUERY_RESPONSES));
  const controller = createDashboardController({ apiUrl: "http://api.test", poolId: "pool-a" });
  await controller.refresh();
  assert.equal(doc.getElementById("btn-retry").hidden, false);
});

test("addAlert with a blank Pool ID does not drop another pool's alert with the same metric", async () => {
  const doc = setupRefreshTestDom();
  installFakeFetch([]);
  doc.registerElement("alert-metric", { value: "tvl" });
  doc.registerElement("alert-threshold", { value: "10" });

  const controller = createDashboardController({ apiUrl: "http://api.test", poolId: "" });

  doc.getElementById("pool-id").value = "pool-a";
  await controller.addAlert();

  doc.getElementById("pool-id").value = "pool-b";
  doc.getElementById("alert-threshold").value = "20";
  await controller.addAlert();

  // Blank Pool ID: previously this dropped both prior entries because the
  // dedup filter only matched on metric, ignoring poolId.
  doc.getElementById("pool-id").value = "";
  doc.getElementById("alert-threshold").value = "30";
  await controller.addAlert();

  assert.equal(doc.getElementById("alert-list").children.length, 3);
});
