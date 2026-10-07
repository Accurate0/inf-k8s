import assert from "node:assert/strict";
import { test } from "node:test";

import type { ExistingTransaction, FireflyTransaction } from "../src/firefly.ts";
import { Reconciler } from "../src/reconciler.ts";

const FETCHED_SINCE = new Date("2026-10-01T00:00:00+08:00");

function planned(overrides: Partial<FireflyTransaction> & { external_id: string }): FireflyTransaction {
  return {
    type: "withdrawal",
    date: "2026-10-05T09:00:00+08:00",
    amount: "10.00",
    description: "Coffee",
    currency_code: "AUD",
    source_id: "1",
    destination_name: "Coffee",
    ...overrides,
  };
}

function existing(overrides: Partial<ExistingTransaction> & { externalId: string }): ExistingTransaction {
  return {
    groupId: "100",
    journalId: "100",
    splits: 1,
    type: "withdrawal",
    date: "2026-10-05T09:00:00+08:00",
    amount: "10.000000000000",
    sourceId: "1",
    destinationId: "50",
    tags: [],
    ...overrides,
  };
}

function reconcile(plan: FireflyTransaction[], current: ExistingTransaction[], upIds = plan.map((p) => p.external_id)) {
  return new Reconciler({
    planned: plan,
    existing: new Map(current.map((entry) => [entry.externalId, entry])),
    upIds: new Set(upIds),
    fetchedSince: FETCHED_SINCE,
    pendingTag: "pending",
  });
}

test("new transactions are created and matching ones are left alone", () => {
  const result = reconcile([planned({ external_id: "a" }), planned({ external_id: "b" })], [existing({ externalId: "a" })]);

  assert.deepEqual(result.create.map((entry) => entry.external_id), ["b"]);
  assert.equal(result.update.length, 0);
  assert.equal(result.remove.length, 0);
  assert.equal(result.unchanged, 1);
});

test("an expense that is really a move between own accounts is converted", () => {
  const result = reconcile(
    [planned({ external_id: "a", type: "transfer", source_id: "1", destination_id: "9", destination_name: undefined })],
    [existing({ externalId: "a" })],
  );

  assert.equal(result.update.length, 1);
  assert.deepEqual(result.update[0]?.update, { type: "transfer", source_id: "1", destination_id: "9" });
});

test("an entry the user re-pointed by hand is not turned back into an expense", () => {
  const result = reconcile(
    [planned({ external_id: "a" })],
    [existing({ externalId: "a", type: "transfer", destinationId: "9" })],
  );

  assert.equal(result.update.length, 0);
  assert.equal(result.unchanged, 1);
});

test("a settled hold loses the pending tag, keeps other tags and takes the final amount", () => {
  const result = reconcile(
    [planned({ external_id: "a", amount: "11.50" })],
    [existing({ externalId: "a", tags: ["pending", "holiday"] })],
  );

  assert.deepEqual(result.update[0]?.update, { tags: ["holiday"], amount: "11.50" });
});

test("a hold that is still pending only changes when its amount does", () => {
  const same = reconcile([planned({ external_id: "a", tags: ["pending"] })], [existing({ externalId: "a", tags: ["pending"] })]);
  const changed = reconcile(
    [planned({ external_id: "a", tags: ["pending"], amount: "12.00" })],
    [existing({ externalId: "a", tags: ["pending"] })],
  );

  assert.equal(same.update.length, 0);
  assert.deepEqual(changed.update[0]?.update, { amount: "12.00" });
});

test("a settled entry's amount is never rewritten", () => {
  const result = reconcile([planned({ external_id: "a", amount: "99.00" })], [existing({ externalId: "a" })]);

  assert.equal(result.update.length, 0);
});

test("a pending entry Up no longer reports is removed", () => {
  const result = reconcile([], [existing({ externalId: "gone", tags: ["pending"] })], []);

  assert.deepEqual(result.remove.map((entry) => entry.externalId), ["gone"]);
});

test("entries are never removed unless they are pending, single and inside the fetched window", () => {
  const result = reconcile(
    [],
    [
      existing({ externalId: "settled" }),
      existing({ externalId: "old", tags: ["pending"], date: "2026-09-20T09:00:00+08:00" }),
      existing({ externalId: "split", tags: ["pending"], splits: 2 }),
      existing({ externalId: "amex-row", tags: [] }),
    ],
    [],
  );

  assert.equal(result.remove.length, 0);
});
