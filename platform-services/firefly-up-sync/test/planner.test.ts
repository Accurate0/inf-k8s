import assert from "node:assert/strict";
import { test } from "node:test";

import { Planner, type TrackedAccount } from "../src/planner.ts";
import type { UpTransaction } from "../src/up.ts";

const ACCOUNTS = new Map<string, TrackedAccount>([
  ["up-spending", { id: "1", kind: "asset" }],
  ["up-saver", { id: "2", kind: "asset" }],
  ["up-home-loan", { id: "3", kind: "liability" }],
]);

const IMPORT_SINCE = new Date("2026-10-01T00:00:00+08:00");

type Overrides = {
  id: string;
  account: string;
  cents: number;
  createdAt?: string;
  description?: string;
  transferAccount?: string;
  category?: string;
  tags?: string[];
  message?: string;
  foreign?: { cents: number; currency: string };
};

function transaction(overrides: Overrides): UpTransaction {
  return {
    id: overrides.id,
    attributes: {
      status: "SETTLED",
      rawText: null,
      description: overrides.description ?? "Coffee",
      message: overrides.message ?? null,
      amount: {
        currencyCode: "AUD",
        value: (overrides.cents / 100).toFixed(2),
        valueInBaseUnits: overrides.cents,
      },
      foreignAmount: overrides.foreign
        ? {
            currencyCode: overrides.foreign.currency,
            value: (overrides.foreign.cents / 100).toFixed(2),
            valueInBaseUnits: overrides.foreign.cents,
          }
        : null,
      createdAt: overrides.createdAt ?? "2026-10-05T09:00:00+08:00",
      settledAt: null,
    },
    relationships: {
      account: { data: { id: overrides.account } },
      transferAccount: { data: overrides.transferAccount ? { id: overrides.transferAccount } : null },
      category: { data: overrides.category ? { id: overrides.category } : null },
      tags: { data: (overrides.tags ?? []).map((id) => ({ id })) },
    },
  };
}

function plan(transactions: UpTransaction[]) {
  return new Planner(ACCOUNTS, IMPORT_SINCE).plan(transactions);
}

test("a purchase becomes a withdrawal to the merchant", () => {
  const [planned] = plan([
    transaction({
      id: "t1",
      account: "up-spending",
      cents: -450,
      category: "restaurants-and-cafes",
      tags: ["work"],
    }),
  ]);

  assert.equal(planned?.type, "withdrawal");
  assert.equal(planned?.amount, "4.50");
  assert.equal(planned?.source_id, "1");
  assert.equal(planned?.destination_name, "Coffee");
  assert.equal(planned?.external_id, "t1");
  assert.equal(planned?.category_name, "Restaurants and cafes");
  assert.deepEqual(planned?.tags, ["work"]);
});

test("incoming money becomes a deposit from the payer", () => {
  const [planned] = plan([transaction({ id: "t1", account: "up-spending", cents: 250000, description: "Salary" })]);

  assert.equal(planned?.type, "deposit");
  assert.equal(planned?.amount, "2500.00");
  assert.equal(planned?.source_name, "Salary");
  assert.equal(planned?.destination_id, "1");
});

test("both sides of a transfer collapse into one transfer keyed on the outgoing side", () => {
  const planned = plan([
    transaction({ id: "in", account: "up-saver", cents: 10000, transferAccount: "up-spending" }),
    transaction({ id: "out", account: "up-spending", cents: -10000, transferAccount: "up-saver" }),
  ]);

  assert.equal(planned.length, 1);
  assert.equal(planned[0]?.type, "transfer");
  assert.equal(planned[0]?.external_id, "out");
  assert.equal(planned[0]?.source_id, "1");
  assert.equal(planned[0]?.destination_id, "2");
});

test("an incoming transfer with no outgoing side is still recorded", () => {
  const planned = plan([
    transaction({ id: "purchase", account: "up-spending", cents: -450 }),
    transaction({ id: "roundup", account: "up-saver", cents: 50, transferAccount: "up-spending" }),
  ]);

  const transfer = planned.find((entry) => entry.type === "transfer");

  assert.equal(planned.length, 2);
  assert.equal(transfer?.external_id, "roundup");
  assert.equal(transfer?.source_id, "1");
  assert.equal(transfer?.destination_id, "2");
});

test("two identical transfers pair up one to one", () => {
  const planned = plan([
    transaction({ id: "out-1", account: "up-spending", cents: -500, transferAccount: "up-saver" }),
    transaction({ id: "in-1", account: "up-saver", cents: 500, transferAccount: "up-spending" }),
    transaction({ id: "out-2", account: "up-spending", cents: -500, transferAccount: "up-saver" }),
    transaction({ id: "in-2", account: "up-saver", cents: 500, transferAccount: "up-spending" }),
  ]);

  assert.deepEqual(planned.map((entry) => entry.external_id).sort(), ["out-1", "out-2"]);
});

test("an outgoing side before the import window still suppresses its incoming side", () => {
  const planned = plan([
    transaction({
      id: "out",
      account: "up-spending",
      cents: -10000,
      transferAccount: "up-saver",
      createdAt: "2026-09-30T23:59:59+08:00",
    }),
    transaction({
      id: "in",
      account: "up-saver",
      cents: 10000,
      transferAccount: "up-spending",
      createdAt: "2026-10-01T00:00:01+08:00",
    }),
  ]);

  assert.equal(planned.length, 0);
});

test("transactions before the import window or on untracked accounts are skipped", () => {
  const planned = plan([
    transaction({ id: "old", account: "up-spending", cents: -100, createdAt: "2026-09-20T09:00:00+08:00" }),
    transaction({ id: "closed", account: "up-closed", cents: -100 }),
  ]);

  assert.equal(planned.length, 0);
});

test("a transfer to an untracked account is treated as a plain withdrawal", () => {
  const [planned] = plan([
    transaction({ id: "t1", account: "up-spending", cents: -5000, transferAccount: "up-closed" }),
  ]);

  assert.equal(planned?.type, "withdrawal");
  assert.equal(planned?.destination_name, "Coffee");
  assert.equal(planned?.destination_id, undefined);
});

test("a home loan repayment is one withdrawal into the liability", () => {
  const planned = plan([
    transaction({ id: "out", account: "up-spending", cents: -357300, transferAccount: "up-home-loan" }),
    transaction({ id: "in", account: "up-home-loan", cents: 357300, transferAccount: "up-spending" }),
  ]);

  assert.equal(planned.length, 1);
  assert.equal(planned[0]?.type, "withdrawal");
  assert.equal(planned[0]?.external_id, "out");
  assert.equal(planned[0]?.source_id, "1");
  assert.equal(planned[0]?.destination_id, "3");
  assert.equal(planned[0]?.destination_name, undefined);
});

test("a redraw from the home loan is one deposit out of the liability", () => {
  const planned = plan([
    transaction({ id: "out", account: "up-home-loan", cents: -100000, transferAccount: "up-spending" }),
    transaction({ id: "in", account: "up-spending", cents: 100000, transferAccount: "up-home-loan" }),
  ]);

  assert.equal(planned.length, 1);
  assert.equal(planned[0]?.type, "deposit");
  assert.equal(planned[0]?.source_id, "3");
  assert.equal(planned[0]?.destination_id, "1");
});

test("interest charged on the home loan is a withdrawal from the liability", () => {
  const [planned] = plan([
    transaction({ id: "t1", account: "up-home-loan", cents: -123456, description: "Interest" }),
  ]);

  assert.equal(planned?.type, "withdrawal");
  assert.equal(planned?.amount, "1234.56");
  assert.equal(planned?.source_id, "3");
  assert.equal(planned?.destination_name, "Interest");
});

test("message and foreign amount end up in the notes", () => {
  const [planned] = plan([
    transaction({
      id: "t1",
      account: "up-spending",
      cents: -1550,
      message: "thanks",
      foreign: { cents: -1000, currency: "USD" },
    }),
  ]);

  assert.equal(planned?.notes, "thanks\nForeign amount: 10.00 USD");
});
