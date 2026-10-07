import type { Config } from "./config.ts";
import type { AccountKind, FireflyClient, FireflyTransaction, NewFireflyAccount } from "./firefly.ts";
import { type LinkedAccount, Planner, type TrackedAccount } from "./planner.ts";
import { type PlannedUpdate, Reconciler } from "./reconciler.ts";
import { Schedule } from "./schedule.ts";
import type { UpAccount, UpClient, UpTransaction } from "./up.ts";

export type SyncResult = {
  fetched: number;
  unchanged: number;
  created: number;
  updated: number;
  removed: number;
};

export class Sync {
  static readonly DAY_MS = 24 * 60 * 60 * 1000;
  static readonly FETCH_MARGIN_DAYS = 1;
  static readonly EXISTING_MARGIN_DAYS = 2;
  static readonly PENDING_ACCOUNT_ID = "pending";

  static readonly ACCOUNT_KINDS = new Map<string, AccountKind>([
    ["TRANSACTIONAL", "asset"],
    ["SAVER", "asset"],
    ["HOME_LOAN", "liability"],
  ]);

  private readonly config: Config;
  private readonly up: UpClient;
  private readonly firefly: FireflyClient;

  constructor(config: Config, up: UpClient, firefly: FireflyClient) {
    this.config = config;
    this.up = up;
    this.firefly = firefly;
  }

  async run(now: Date): Promise<SyncResult> {
    const importSince = new Date(now.getTime() - this.config.lookbackDays * Sync.DAY_MS);
    const fetchSince = new Date(importSince.getTime() - Sync.FETCH_MARGIN_DAYS * Sync.DAY_MS);

    const transactions = await this.up.transactionsSince(fetchSince);
    const accounts = await this.resolveAccounts(transactions, importSince);
    const linked = await this.resolveLinked();

    const planned = new Planner(accounts, importSince, linked).plan(transactions);

    const existing = await this.firefly.existingBetween(
      new Date(fetchSince.getTime() - Sync.EXISTING_MARGIN_DAYS * Sync.DAY_MS),
      new Date(now.getTime() + Sync.EXISTING_MARGIN_DAYS * Sync.DAY_MS),
    );

    const reconciler = new Reconciler({
      planned,
      existing,
      upIds: new Set(transactions.map((transaction) => transaction.id)),
      fetchedSince: fetchSince,
      pendingTag: Planner.PENDING_TAG,
    });

    const schedule = new Schedule(reconciler.create, this.config.concurrency);

    await this.createAll(schedule.warmup);
    await Promise.all(schedule.lanes.map((lane) => this.createAll(lane)));

    const updates = reconciler.update.filter((entry) => this.config.dryRun || !Sync.touchesPendingAccount(entry));

    await this.updateAll(updates);
    await this.removeAll(reconciler);

    return {
      fetched: transactions.length,
      unchanged: reconciler.unchanged,
      created: this.config.dryRun ? 0 : reconciler.create.length,
      updated: this.config.dryRun ? 0 : updates.length,
      removed: this.config.dryRun ? 0 : reconciler.remove.length,
    };
  }

  private static touchesPendingAccount(entry: PlannedUpdate): boolean {
    return (
      entry.update.source_id === Sync.PENDING_ACCOUNT_ID || entry.update.destination_id === Sync.PENDING_ACCOUNT_ID
    );
  }

  private static fireflyName(account: UpAccount): string {
    return `Up - ${account.attributes.displayName}`;
  }

  private static balanceBefore(account: UpAccount, transactions: UpTransaction[], importSince: Date): number {
    let cents = account.attributes.balance.valueInBaseUnits;

    for (const transaction of transactions) {
      if (transaction.relationships.account.data.id !== account.id) {
        continue;
      }

      if (new Date(transaction.attributes.createdAt) < importSince) {
        continue;
      }

      cents -= transaction.attributes.amount.valueInBaseUnits;
    }

    return cents;
  }

  private static newAccount(
    account: UpAccount,
    kind: AccountKind,
    transactions: UpTransaction[],
    importSince: Date,
  ): NewFireflyAccount {
    const common = {
      name: Sync.fireflyName(account),
      currency_code: account.attributes.balance.currencyCode,
      account_number: account.id,
    };

    if (kind === "asset") {
      return {
        ...common,
        type: "asset",
        account_role: account.attributes.accountType === "SAVER" ? "savingAsset" : "defaultAsset",
      };
    }

    const owedCents = -Sync.balanceBefore(account, transactions, importSince);

    if (owedCents <= 0) {
      return {
        ...common,
        type: "liability",
        liability_type: "mortgage",
        liability_direction: "debit",
      };
    }

    return {
      ...common,
      type: "liability",
      liability_type: "mortgage",
      liability_direction: "debit",
      opening_balance: (owedCents / 100).toFixed(2),
      opening_balance_date: importSince.toISOString().slice(0, 10),
    };
  }

  private async createAll(transactions: FireflyTransaction[]): Promise<void> {
    for (const transaction of transactions) {
      console.log(
        `${this.config.dryRun ? "would create" : "creating"} ${transaction.type} ` +
          `${transaction.amount} ${transaction.currency_code} "${transaction.description}" (${transaction.external_id})`,
      );

      if (!this.config.dryRun) {
        await this.firefly.createTransaction(transaction);
      }
    }
  }

  private async updateAll(updates: PlannedUpdate[]): Promise<void> {
    for (const { existing, update, reason } of updates) {
      console.log(`${this.config.dryRun ? "would update" : "updating"} #${existing.groupId}: ${reason} (${existing.externalId})`);

      if (!this.config.dryRun) {
        await this.firefly.updateTransaction(existing, update);
      }
    }
  }

  private async removeAll(reconciler: Reconciler): Promise<void> {
    for (const existing of reconciler.remove) {
      console.log(
        `${this.config.dryRun ? "would remove" : "removing"} #${existing.groupId}: hold released by Up (${existing.externalId})`,
      );

      if (!this.config.dryRun) {
        await this.firefly.deleteTransaction(existing);
      }
    }
  }

  private async resolveLinked(): Promise<LinkedAccount[]> {
    if (this.config.linkedAccounts.length === 0) {
      return [];
    }

    const byName = new Map<string, string>();

    for (const account of await this.firefly.accounts("asset")) {
      byName.set(account.attributes.name, account.id);
    }

    const linked: LinkedAccount[] = [];

    for (const { name, matches, ids } of this.config.linkedAccounts) {
      const existingId = byName.get(name);

      if (existingId) {
        linked.push({ matches, ids, account: { id: existingId, kind: "asset" } });
        continue;
      }

      if (this.config.dryRun) {
        console.log(`would create Firefly account "${name}"`);
        linked.push({ matches, ids, account: { id: Sync.PENDING_ACCOUNT_ID, kind: "asset" } });
        continue;
      }

      const created = await this.firefly.createAccount({
        name,
        type: "asset",
        account_role: "defaultAsset",
        currency_code: "AUD",
      });

      console.log(`created Firefly account "${created.attributes.name}" (#${created.id})`);
      linked.push({ matches, ids, account: { id: created.id, kind: "asset" } });
    }

    return linked;
  }

  private async resolveAccounts(
    transactions: UpTransaction[],
    importSince: Date,
  ): Promise<Map<string, TrackedAccount>> {
    const upAccounts = await this.up.accounts();

    const existing = new Map<string, TrackedAccount>();

    for (const kind of new Set(Sync.ACCOUNT_KINDS.values())) {
      for (const account of await this.firefly.accounts(kind)) {
        if (account.attributes.account_number) {
          existing.set(account.attributes.account_number, { id: account.id, kind });
        }
      }
    }

    const resolved = new Map<string, TrackedAccount>();

    for (const account of upAccounts) {
      const kind = Sync.ACCOUNT_KINDS.get(account.attributes.accountType);

      if (!kind) {
        console.log(`skipping ${account.attributes.accountType} account "${account.attributes.displayName}"`);
        continue;
      }

      const tracked = existing.get(account.id);

      if (tracked) {
        resolved.set(account.id, tracked);
        continue;
      }

      const newAccount = Sync.newAccount(account, kind, transactions, importSince);
      const opening = "opening_balance" in newAccount ? `, owing ${newAccount.opening_balance}` : "";

      if (this.config.dryRun) {
        console.log(`would create Firefly ${kind} account "${newAccount.name}"${opening}`);
        resolved.set(account.id, { id: Sync.PENDING_ACCOUNT_ID, kind });
        continue;
      }

      const created = await this.firefly.createAccount(newAccount);

      console.log(`created Firefly ${kind} account "${created.attributes.name}" (#${created.id})${opening}`);
      resolved.set(account.id, { id: created.id, kind });
    }

    return resolved;
  }
}
