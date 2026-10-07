import type { AccountKind, FireflyTransaction } from "./firefly.ts";
import type { UpTransaction } from "./up.ts";

export type TrackedAccount = {
  id: string;
  kind: AccountKind;
};

export type LinkedAccount = {
  matches: string[];
  ids: string[];
  account: TrackedAccount;
};

export class Planner {
  static readonly COUNTERPART_WINDOW_MS = 60_000;
  static readonly PENDING_TAG = "pending";

  private readonly accounts: Map<string, TrackedAccount>;
  private readonly importSince: Date;
  private readonly linked: LinkedAccount[];

  constructor(accounts: Map<string, TrackedAccount>, importSince: Date, linked: LinkedAccount[] = []) {
    this.accounts = accounts;
    this.importSince = importSince;
    this.linked = linked.map((entry) => ({ ...entry, matches: entry.matches.map((match) => match.toLowerCase()) }));
  }

  plan(transactions: UpTransaction[]): FireflyTransaction[] {
    const claimedOutgoing = new Set<string>();
    const planned: FireflyTransaction[] = [];

    for (const transaction of transactions) {
      if (!this.isTracked(transaction)) {
        continue;
      }

      if (this.isInternalTransfer(transaction) && Planner.cents(transaction) > 0) {
        const counterpart = this.findOutgoingCounterpart(transaction, transactions, claimedOutgoing);

        if (counterpart) {
          claimedOutgoing.add(counterpart.id);
          continue;
        }
      }

      if (new Date(transaction.attributes.createdAt) < this.importSince) {
        continue;
      }

      if (Planner.cents(transaction) === 0) {
        continue;
      }

      planned.push(this.toFirefly(transaction));
    }

    return planned.sort((a, b) => a.date.localeCompare(b.date));
  }

  private static cents(transaction: UpTransaction): number {
    return transaction.attributes.amount.valueInBaseUnits;
  }

  private static formatAmount(cents: number): string {
    return (Math.abs(cents) / 100).toFixed(2);
  }

  private static categoryName(categoryId: string): string {
    const words = categoryId.replaceAll("-", " ");

    return words.charAt(0).toUpperCase() + words.slice(1);
  }

  private static notes(transaction: UpTransaction): string | undefined {
    const { rawText, message, foreignAmount } = transaction.attributes;
    const lines: string[] = [];

    if (message) {
      lines.push(message);
    }

    if (rawText) {
      lines.push(`Raw: ${rawText}`);
    }

    if (foreignAmount) {
      const value = Planner.formatAmount(foreignAmount.valueInBaseUnits);
      lines.push(`Foreign amount: ${value} ${foreignAmount.currencyCode}`);
    }

    return lines.length > 0 ? lines.join("\n") : undefined;
  }

  private static internalType(source: TrackedAccount, destination: TrackedAccount): FireflyTransaction["type"] {
    if (source.kind === destination.kind) {
      return "transfer";
    }

    return source.kind === "asset" ? "withdrawal" : "deposit";
  }

  private static tags(transaction: UpTransaction): string[] | undefined {
    const tags = transaction.relationships.tags.data.map((tag) => tag.id);

    if (transaction.attributes.status === "HELD") {
      tags.push(Planner.PENDING_TAG);
    }

    return tags.length > 0 ? tags : undefined;
  }

  private linkedFor(transaction: UpTransaction): TrackedAccount | undefined {
    const description = transaction.attributes.description.toLowerCase();

    const entry = this.linked.find(
      (candidate) =>
        candidate.ids.includes(transaction.id) || candidate.matches.some((match) => description.includes(match)),
    );

    return entry?.account;
  }

  private isTracked(transaction: UpTransaction): boolean {
    return this.accounts.has(transaction.relationships.account.data.id);
  }

  private isInternalTransfer(transaction: UpTransaction): boolean {
    const otherId = transaction.relationships.transferAccount.data?.id;

    return otherId !== undefined && this.accounts.has(otherId);
  }

  private findOutgoingCounterpart(
    incoming: UpTransaction,
    candidates: UpTransaction[],
    claimed: Set<string>,
  ): UpTransaction | undefined {
    const accountId = incoming.relationships.account.data.id;
    const otherId = incoming.relationships.transferAccount.data?.id;
    const createdAt = new Date(incoming.attributes.createdAt).getTime();

    return candidates.find((candidate) => {
      if (claimed.has(candidate.id)) {
        return false;
      }

      if (candidate.relationships.account.data.id !== otherId) {
        return false;
      }

      if (candidate.relationships.transferAccount.data?.id !== accountId) {
        return false;
      }

      if (Planner.cents(candidate) !== -Planner.cents(incoming)) {
        return false;
      }

      const candidateCreatedAt = new Date(candidate.attributes.createdAt).getTime();

      return Math.abs(candidateCreatedAt - createdAt) <= Planner.COUNTERPART_WINDOW_MS;
    });
  }

  private toFirefly(transaction: UpTransaction): FireflyTransaction {
    const { attributes, relationships } = transaction;
    const cents = Planner.cents(transaction);
    const account = this.accounts.get(relationships.account.data.id)!;

    const base = {
      date: attributes.createdAt,
      amount: Planner.formatAmount(cents),
      description: attributes.description,
      currency_code: attributes.amount.currencyCode,
      external_id: transaction.id,
      category_name: relationships.category.data ? Planner.categoryName(relationships.category.data.id) : undefined,
      tags: Planner.tags(transaction),
      notes: Planner.notes(transaction),
    };

    if (this.isInternalTransfer(transaction)) {
      const other = this.accounts.get(relationships.transferAccount.data!.id)!;
      const source = cents < 0 ? account : other;
      const destination = cents < 0 ? other : account;

      return {
        ...base,
        type: Planner.internalType(source, destination),
        source_id: source.id,
        destination_id: destination.id,
      };
    }

    const linked = this.linkedFor(transaction);

    if (linked) {
      const source = cents < 0 ? account : linked;
      const destination = cents < 0 ? linked : account;

      return {
        ...base,
        type: Planner.internalType(source, destination),
        source_id: source.id,
        destination_id: destination.id,
      };
    }

    if (cents < 0) {
      return {
        ...base,
        type: "withdrawal",
        source_id: account.id,
        destination_name: attributes.description,
      };
    }

    return {
      ...base,
      type: "deposit",
      source_name: attributes.description,
      destination_id: account.id,
    };
  }
}
