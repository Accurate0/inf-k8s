import type { FireflyTransaction } from "./firefly.ts";

export class Schedule {
  readonly warmup: FireflyTransaction[];
  readonly lanes: FireflyTransaction[][];

  constructor(transactions: FireflyTransaction[], concurrency: number) {
    this.warmup = [];
    this.lanes = Array.from({ length: concurrency }, () => []);

    const seenLabels = new Set<string>();

    for (const transaction of transactions) {
      const labels = Schedule.labels(transaction);

      if (concurrency > 1 && labels.some((label) => !seenLabels.has(label))) {
        labels.forEach((label) => seenLabels.add(label));
        this.warmup.push(transaction);
        continue;
      }

      const lane = Schedule.hash(Schedule.counterparty(transaction)) % concurrency;
      this.lanes[lane]!.push(transaction);
    }
  }

  private static labels(transaction: FireflyTransaction): string[] {
    const labels = (transaction.tags ?? []).map((tag) => `tag:${tag}`);

    if (transaction.category_name) {
      labels.push(`category:${transaction.category_name}`);
    }

    return labels;
  }

  private static counterparty(transaction: FireflyTransaction): string {
    return transaction.source_name ?? transaction.destination_name ?? "";
  }

  private static hash(value: string): number {
    let hash = 0;

    for (let index = 0; index < value.length; index += 1) {
      hash = (hash * 31 + value.charCodeAt(index)) >>> 0;
    }

    return hash;
  }
}
