import assert from "node:assert/strict";
import { test } from "node:test";

import type { FireflyTransaction } from "../src/firefly.ts";
import { Schedule } from "../src/schedule.ts";

function withdrawal(id: string, payee: string, category?: string, tags?: string[]): FireflyTransaction {
  return {
    type: "withdrawal",
    date: "2026-10-05T09:00:00+08:00",
    amount: "1.00",
    description: payee,
    currency_code: "AUD",
    external_id: id,
    source_id: "1",
    destination_name: payee,
    category_name: category,
    tags,
  };
}

function ids(transactions: FireflyTransaction[]): string[] {
  return transactions.map((transaction) => transaction.external_id);
}

test("a single lane keeps everything in order with no warmup", () => {
  const schedule = new Schedule(
    [withdrawal("a", "Coffee", "Food"), withdrawal("b", "Fuel", "Car"), withdrawal("c", "Coffee", "Food")],
    1,
  );

  assert.deepEqual(ids(schedule.warmup), []);
  assert.deepEqual(ids(schedule.lanes[0]!), ["a", "b", "c"]);
});

test("the first use of each category or tag runs in the warmup", () => {
  const schedule = new Schedule(
    [
      withdrawal("a", "Coffee", "Food"),
      withdrawal("b", "Bakery", "Food"),
      withdrawal("c", "Fuel", "Car", ["work"]),
      withdrawal("d", "Tolls", "Car", ["work"]),
      withdrawal("e", "Parking", "Car", ["holiday"]),
      withdrawal("f", "Rent"),
    ],
    4,
  );

  assert.deepEqual(ids(schedule.warmup), ["a", "c", "e"]);
  assert.deepEqual(ids(schedule.lanes.flat()).sort(), ["b", "d", "f"]);
});

test("entries for the same payee always share a lane", () => {
  const payees = ["Coffee", "Fuel", "Rent", "Bakery", "Tolls", "Parking", "Gym"];
  const transactions = Array.from({ length: 70 }, (_, index) => withdrawal(`t${index}`, payees[index % payees.length]!));

  const schedule = new Schedule(transactions, 4);
  const laneOf = new Map<string, number>();

  schedule.lanes.forEach((lane, index) => {
    for (const transaction of lane) {
      const payee = transaction.destination_name!;

      assert.equal(laneOf.get(payee) ?? index, index);
      laneOf.set(payee, index);
    }
  });

  assert.equal(schedule.lanes.flat().length, 70);
  assert.ok(schedule.lanes.filter((lane) => lane.length > 0).length > 1);
});
