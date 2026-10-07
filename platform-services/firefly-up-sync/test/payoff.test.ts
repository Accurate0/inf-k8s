import assert from "node:assert/strict";
import { test } from "node:test";

import type { PiggyBank } from "../src/firefly.ts";
import { Payoff } from "../src/payoff.ts";
import type { TrackedAccount } from "../src/planner.ts";
import type { UpAccount } from "../src/up.ts";

const TRACKED = new Map<string, TrackedAccount>([
  ["up-spending", { id: "7", kind: "asset" }],
  ["up-offset", { id: "8", kind: "asset" }],
  ["up-loan", { id: "14", kind: "liability" }],
]);

function upAccount(id: string, cents: number): UpAccount {
  return {
    id,
    attributes: {
      displayName: id,
      accountType: "SAVER",
      ownershipType: "INDIVIDUAL",
      balance: { currencyCode: "AUD", value: (cents / 100).toFixed(2), valueInBaseUnits: cents },
    },
  };
}

function piggyBank(target: string, saved: string): PiggyBank {
  return {
    id: "1",
    attributes: { name: "Home loan payoff", target_amount: target, accounts: [{ account_id: "8", current_amount: saved }] },
  };
}

const UP = [upAccount("up-spending", 10873), upAccount("up-offset", 30254644), upAccount("up-loan", -56942600)];

test("target follows what is owed and saved follows the offset balance", () => {
  const payoff = new Payoff(piggyBank("570000.00", "300000.00"), UP, TRACKED);

  assert.deepEqual(payoff.update, {
    target_amount: "569426.00",
    accounts: [{ account_id: "8", current_amount: "302546.44" }],
  });
});

test("nothing is updated when the piggy bank already matches", () => {
  const payoff = new Payoff(piggyBank("569426.00", "302546.44"), UP, TRACKED);

  assert.equal(payoff.update, undefined);
});

test("the offset never counts for more than is owed", () => {
  const up = [upAccount("up-offset", 70000000), upAccount("up-loan", -56942600)];
  const payoff = new Payoff(piggyBank("569426.00", "302546.44"), up, TRACKED);

  assert.equal(payoff.update?.accounts[0]?.current_amount, "569426.00");
});

test("a paid-off loan leaves a zero target and nothing saved", () => {
  const up = [upAccount("up-offset", 30254644), upAccount("up-loan", 0)];
  const payoff = new Payoff(piggyBank("569426.00", "302546.44"), up, TRACKED);

  assert.deepEqual(payoff.update, { target_amount: "0.00", accounts: [{ account_id: "8", current_amount: "0.00" }] });
});
