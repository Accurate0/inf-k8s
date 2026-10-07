import type { Config } from "./config.ts";
import type { FireflyClient } from "./firefly.ts";
import { Planner } from "./planner.ts";
import type { UpAccount, UpClient } from "./up.ts";

export type SyncResult = {
  fetched: number;
  alreadyImported: number;
  created: number;
};

export class Sync {
  static readonly DAY_MS = 24 * 60 * 60 * 1000;
  static readonly FETCH_MARGIN_DAYS = 1;
  static readonly EXISTING_MARGIN_DAYS = 2;
  static readonly SUPPORTED_ACCOUNT_TYPES = new Set(["TRANSACTIONAL", "SAVER"]);

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

    const fireflyAccountIds = await this.resolveAccounts();

    const transactions = await this.up.settledTransactionsSince(fetchSince);
    const planned = new Planner(fireflyAccountIds, importSince).plan(transactions);

    const existing = await this.firefly.externalIdsBetween(
      new Date(fetchSince.getTime() - Sync.EXISTING_MARGIN_DAYS * Sync.DAY_MS),
      new Date(now.getTime() + Sync.EXISTING_MARGIN_DAYS * Sync.DAY_MS),
    );

    const missing = planned.filter((transaction) => !existing.has(transaction.external_id));

    for (const transaction of missing) {
      console.log(
        `${this.config.dryRun ? "would create" : "creating"} ${transaction.type} ` +
          `${transaction.amount} ${transaction.currency_code} "${transaction.description}" (${transaction.external_id})`,
      );

      if (!this.config.dryRun) {
        await this.firefly.createTransaction(transaction);
      }
    }

    return {
      fetched: transactions.length,
      alreadyImported: planned.length - missing.length,
      created: this.config.dryRun ? 0 : missing.length,
    };
  }

  private static fireflyName(account: UpAccount): string {
    return `Up - ${account.attributes.displayName}`;
  }

  private async resolveAccounts(): Promise<Map<string, string>> {
    const upAccounts = await this.up.accounts();
    const fireflyAccounts = await this.firefly.assetAccounts();

    const byAccountNumber = new Map<string, string>();

    for (const account of fireflyAccounts) {
      if (account.attributes.account_number) {
        byAccountNumber.set(account.attributes.account_number, account.id);
      }
    }

    const resolved = new Map<string, string>();

    for (const account of upAccounts) {
      if (!Sync.SUPPORTED_ACCOUNT_TYPES.has(account.attributes.accountType)) {
        console.log(`skipping ${account.attributes.accountType} account "${account.attributes.displayName}"`);
        continue;
      }

      const existingId = byAccountNumber.get(account.id);

      if (existingId) {
        resolved.set(account.id, existingId);
        continue;
      }

      if (this.config.dryRun) {
        console.log(`would create Firefly account "${Sync.fireflyName(account)}"`);
        continue;
      }

      const created = await this.firefly.createAccount({
        name: Sync.fireflyName(account),
        type: "asset",
        account_role: account.attributes.accountType === "SAVER" ? "savingAsset" : "defaultAsset",
        currency_code: "AUD",
        account_number: account.id,
      });

      console.log(`created Firefly account "${created.attributes.name}" (#${created.id})`);
      resolved.set(account.id, created.id);
    }

    return resolved;
  }
}
