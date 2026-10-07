import type { ExistingTransaction, FireflyTransaction, TransactionUpdate } from "./firefly.ts";

export type PlannedUpdate = {
  existing: ExistingTransaction;
  update: TransactionUpdate;
  reason: string;
};

export type ReconcileInput = {
  planned: FireflyTransaction[];
  existing: Map<string, ExistingTransaction>;
  upIds: Set<string>;
  fetchedSince: Date;
  pendingTag: string;
};

export class Reconciler {
  readonly create: FireflyTransaction[];
  readonly update: PlannedUpdate[];
  readonly remove: ExistingTransaction[];
  readonly unchanged: number;

  constructor(input: ReconcileInput) {
    this.create = [];
    this.update = [];
    this.remove = [];

    let unchanged = 0;

    for (const transaction of input.planned) {
      const existing = input.existing.get(transaction.external_id);

      if (!existing) {
        this.create.push(transaction);
        continue;
      }

      const change = Reconciler.change(transaction, existing, input.pendingTag);

      if (!change) {
        unchanged += 1;
        continue;
      }

      this.update.push({ existing, ...change });
    }

    for (const existing of input.existing.values()) {
      if (Reconciler.isReleasedHold(existing, input)) {
        this.remove.push(existing);
      }
    }

    this.unchanged = unchanged;
  }

  private static isReleasedHold(existing: ExistingTransaction, input: ReconcileInput): boolean {
    if (!existing.tags.includes(input.pendingTag)) {
      return false;
    }

    if (existing.splits !== 1) {
      return false;
    }

    if (new Date(existing.date) < input.fetchedSince) {
      return false;
    }

    return !input.upIds.has(existing.externalId);
  }

  private static change(
    transaction: FireflyTransaction,
    existing: ExistingTransaction,
    pendingTag: string,
  ): { update: TransactionUpdate; reason: string } | undefined {
    const update: TransactionUpdate = {};
    const reasons: string[] = [];

    const wasPending = existing.tags.includes(pendingTag);
    const isPending = transaction.tags?.includes(pendingTag) ?? false;

    if (wasPending && !isPending) {
      update.tags = existing.tags.filter((tag) => tag !== pendingTag);
      reasons.push("settled");
    }

    if (wasPending && Number(existing.amount) !== Number(transaction.amount)) {
      update.amount = transaction.amount;
      reasons.push(`amount ${Number(existing.amount).toFixed(2)} -> ${transaction.amount}`);
    }

    if (Reconciler.linksOwnAccounts(transaction) && Reconciler.structureDiffers(transaction, existing)) {
      update.type = transaction.type;
      update.source_id = transaction.source_id;
      update.destination_id = transaction.destination_id;
      reasons.push(`${existing.type} -> ${transaction.type} between own accounts`);
    }

    if (reasons.length === 0) {
      return undefined;
    }

    return { update, reason: reasons.join(", ") };
  }

  private static linksOwnAccounts(transaction: FireflyTransaction): boolean {
    return transaction.source_id !== undefined && transaction.destination_id !== undefined;
  }

  private static structureDiffers(transaction: FireflyTransaction, existing: ExistingTransaction): boolean {
    return (
      existing.type !== transaction.type ||
      existing.sourceId !== transaction.source_id ||
      existing.destinationId !== transaction.destination_id
    );
  }
}
