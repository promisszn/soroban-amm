// Tests for the event emit-site extractor and the schema drift check.
// Run with: node --test scripts/check_event_schemas.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { extractEmitSites, extractFromSource } from "./event_emit_sites.mjs";
import { findProblems, LOCAL_NAME_ALIASES, renderCatalogue } from "./check_event_schemas.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const schema = () => JSON.parse(readFileSync(join(ROOT, "services/webhook-streamer/src/event-schemas.json"), "utf8"));
const docs = () => readFileSync(join(ROOT, "docs/event-schema-versioning.md"), "utf8");

const site = (src) => extractFromSource(src, "x.rs", "demo");

// ── Extractor ────────────────────────────────────────────────────────────

test("reads a versioned emit with a symbol_short! name", () => {
  const [s] = site(`fn f() { emit_versioned_event!(env, (symbol_short!("hello"), who), (a, b, c)); }`);
  assert.deepEqual([s.event, s.topics, s.arity, s.shape, s.versioned], ["hello", 2, 3, "tuple", true]);
  assert.deepEqual(s.args, ["a", "b", "c"]);
});

test("reads Symbol::new names and a one-element tuple", () => {
  const [s] = site(`soroban_amm_sdk::emit_versioned_event!(env, (Symbol::new(&env, "ev"),), (now,));`);
  assert.deepEqual([s.event, s.topics, s.arity, s.shape], ["ev", 1, 1, "tuple"]);
});

test("treats a parenthesised single value as a bare value, not a tuple", () => {
  const [s] = site(`emit_versioned_event!(env, (symbol_short!("ev"),), (value));`);
  assert.deepEqual([s.arity, s.shape], [1, "scalar"]);
});

test("captures plain payload identifiers and falls back per element for anything else", () => {
  const [plain, mixed] = site(`
    emit_versioned_event!(env, (symbol_short!("plain"),), (campaign_id, amount));
    emit_versioned_event!(env, (symbol_short!("mixed"),), (provider.clone(), price as i128, true, MAX_BPS, fee));
  `);
  assert.deepEqual(plain.payload_fields, ["campaign_id", "amount"]);
  assert.deepEqual(mixed.payload_fields, [null, null, null, null, "fee"]);
});

test("counts commas only at the top level", () => {
  const [s] = site(`emit_versioned_event!(env, (symbol_short!("ev"),), (f(a, b), (c, d), [e, f]));`);
  assert.equal(s.arity, 3);
});

test("recognises a raw publish with its own version, and an unversioned one", () => {
  const sites = site(`
    env.events().publish((symbol_short!("a"),), (EVENT_SCHEMA_VERSION, (x, y)));
    env.events().publish((symbol_short!("b"),), (x, y));
  `);
  assert.deepEqual(sites.map((s) => [s.event, s.arity, s.versioned]), [["a", 2, true], ["b", 2, false]]);
});

test("resolves a named constant used as the event name", () => {
  const [s] = site(`
    const EV_THING: &str = "thing";
    fn f() { emit_versioned_event!(env, (Symbol::new(&env, EV_THING),), (a,)); }
  `);
  assert.equal(s.event, "thing");
});

test("ignores comments and test modules", () => {
  const sites = site(`
    // emit_versioned_event!(env, (symbol_short!("commented"),), (a,));
    /* emit_versioned_event!(env, (symbol_short!("block"),), (a,)); */
    #[cfg(test)]
    mod tests { fn t() { emit_versioned_event!(env, (symbol_short!("in_test"),), (a,)); } }
    fn real() { emit_versioned_event!(env, (symbol_short!("real"),), (a,)); }
  `);
  assert.deepEqual(sites.map((s) => s.event), ["real"]);
});

test("skips macro definitions", () => {
  assert.deepEqual(site(`$env.events().publish($topic, ($crate::EVENT_SCHEMA_VERSION, $payload));`), []);
});

// ── The repository ───────────────────────────────────────────────────────

test("the schema file matches every contract emit site and the generated docs are current", () => {
  const problems = findProblems(schema(), extractEmitSites(), docs());
  assert.deepEqual(problems, []);
});

test("every deployed contract that emits events has a schema", () => {
  const emitting = new Set(extractEmitSites().map((s) => s.contract));
  assert.deepEqual([...emitting].sort(), Object.keys(schema().kinds).sort());
});

// ── Drift is caught ──────────────────────────────────────────────────────

test("fails when an emit site gains a payload field the schema lacks", () => {
  const sites = extractEmitSites();
  const flash = sites.find((s) => s.contract === "amm" && s.event === "flash_loan");
  assert.ok(flash);
  const drifted = sites.map((s) => (s === flash ? { ...s, arity: flash.arity + 1 } : s));
  const problems = findProblems(schema(), drifted, docs());
  const hit = problems.find((p) => /amm\.flash_loan is emitted with 5 payload field/.test(p));
  assert.ok(hit, problems.join("\n"));
  assert.match(hit, /only describes: 4 field\(s\)\/2 topic\(s\) versioned$/);
});

test("fails when a plain payload field name differs from the schema", () => {
  // #1323: the schema called reward_distributed's amount field `pending`.
  const s = schema();
  const reward = s.kinds.incentive_campaigns.events.reward_distributed[0];
  assert.equal(reward.fields[2], "amount");
  reward.fields[2] = "pending";
  const problems = findProblems(s, extractEmitSites(), docs());
  const hit = problems.filter((p) => /incentive_campaigns\.reward_distributed payload names disagree/.test(p));
  assert.equal(hit.length, 1, problems.join("\n"));
  assert.match(hit[0], /field 2 is built from `amount` but the schema names it `pending`/);
  // A name mismatch is reported once, not again as an unemitted schema entry.
  assert.ok(!problems.some((p) => /reward_distributed .* is not emitted/.test(p)), problems.join("\n"));
});

test("fails when an emit site renames a plain payload identifier", () => {
  const sites = extractEmitSites();
  const pruned = sites.find((s) => s.contract === "twap_consumer" && s.event === "pruned");
  assert.deepEqual(pruned.payload_fields, ["removed_count", "oldest_remaining_ts"]);
  const drifted = sites.map((s) => (s === pruned ? { ...s, payload_fields: ["remove_count", "oldest_remaining_ts"] } : s));
  const problems = findProblems(schema(), drifted, docs());
  assert.ok(problems.some((p) => /twap_consumer\.pruned payload names disagree.*`remove_count`.*`removed_count`/.test(p)), problems.join("\n"));
});

test("fails when two plain payload fields swap places", () => {
  const sites = extractEmitSites();
  const reward = sites.find((s) => s.contract === "incentive_campaigns" && s.event === "reward_distributed");
  const [a, b, ...rest] = reward.payload_fields;
  const drifted = sites.map((s) => (s === reward ? { ...s, payload_fields: [b, a, ...rest] } : s));
  const problems = findProblems(schema(), drifted, docs());
  assert.ok(problems.some((p) => /incentive_campaigns\.reward_distributed payload names disagree/.test(p)), problems.join("\n"));
});

test("a recorded local-name alias is accepted, but only for its own field", () => {
  const sites = extractEmitSites();
  const recovered = sites.find((s) => s.contract === "amm" && s.event === "cb_recovered");
  assert.deepEqual(recovered.payload_fields, ["now"]);
  assert.deepEqual(findProblems(schema(), sites, docs()), []);
  const problems = findProblems(schema(), sites, docs(), { ...LOCAL_NAME_ALIASES, "amm.cb_recovered": { now: "other" } });
  assert.ok(problems.some((p) => /amm\.cb_recovered payload names disagree/.test(p)), problems.join("\n"));
});

test("fails when a local-name alias is no longer used by any emit site", () => {
  const aliases = { ...LOCAL_NAME_ALIASES, "amm.swap": { gone: "amount_in" } };
  const problems = findProblems(schema(), extractEmitSites(), docs(), aliases);
  assert.ok(problems.some((p) => /LOCAL_NAME_ALIASES maps amm\.swap `gone` -> `amount_in` but no emit site uses it/.test(p)), problems.join("\n"));
});

test("fails when a contract starts emitting an event the schema does not know", () => {
  const sites = [...extractEmitSites(), { contract: "amm", event: "brand_new", topics: 1, arity: 2, shape: "tuple", versioned: true, args: [], source: "contracts/amm/src/lib.rs:1" }];
  const problems = findProblems(schema(), sites, docs());
  assert.ok(problems.some((p) => /emits "brand_new" but event-schemas.json has no entry/.test(p)));
});

test("fails when the schema keeps an event no contract emits any more", () => {
  const sites = extractEmitSites().filter((s) => !(s.contract === "amm" && s.event === "flash_loan"));
  const problems = findProblems(schema(), sites, docs());
  assert.ok(problems.some((p) => /amm\.flash_loan .* is not emitted by any contract/.test(p)));
});

test("fails when the generated docs are stale", () => {
  const s = schema();
  s.kinds.amm.events.swap[0].fields[0] = "renamed";
  const problems = findProblems(s, extractEmitSites(), docs());
  assert.ok(problems.some((p) => /catalogue is out of date/.test(p)));
});

test("the generated catalogue lists the AMM swap fields", () => {
  assert.match(renderCatalogue(schema()), /\| `swap` \| 2 \| `token_in`, `amount_in`, `token_out`, `amount_out` \| v1 \|/);
});
