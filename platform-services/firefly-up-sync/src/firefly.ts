export type FireflyAccount = {
  id: string;
  attributes: {
    name: string;
    account_number: string | null;
  };
};

export type AccountKind = "asset" | "liability";

export type NewFireflyAccount =
  | {
      name: string;
      type: "asset";
      account_role: "defaultAsset" | "savingAsset";
      currency_code: string;
      account_number?: string;
    }
  | {
      name: string;
      type: "liability";
      liability_type: "mortgage";
      liability_direction: "debit";
      currency_code: string;
      account_number: string;
      opening_balance?: string;
      opening_balance_date?: string;
    };

export type FireflyTransaction = {
  type: "withdrawal" | "deposit" | "transfer";
  date: string;
  amount: string;
  description: string;
  currency_code: string;
  external_id: string;
  source_id?: string;
  source_name?: string;
  destination_id?: string;
  destination_name?: string;
  category_name?: string;
  tags?: string[];
  notes?: string;
};

type FireflyPage<T> = {
  data: T[];
  meta: { pagination: { total_pages: number } };
};

export type PiggyBank = {
  id: string;
  attributes: {
    name: string;
    target_amount: string;
    accounts: { account_id: string; current_amount: string }[];
  };
};

export type PiggyBankUpdate = {
  target_amount: string;
  accounts: { account_id: string; current_amount: string }[];
};

export type ExistingTransaction = {
  groupId: string;
  journalId: string;
  splits: number;
  externalId: string;
  type: string;
  date: string;
  amount: string;
  sourceId: string;
  destinationId: string;
  tags: string[];
};

export type TransactionUpdate = {
  type?: FireflyTransaction["type"];
  date?: string;
  amount?: string;
  source_id?: string;
  destination_id?: string;
  tags?: string[];
};

type FireflyTransactionGroup = {
  id: string;
  attributes: {
    transactions: {
      transaction_journal_id: string;
      external_id: string | null;
      type: string;
      date: string;
      amount: string;
      source_id: string;
      destination_id: string;
      tags: string[] | null;
    }[];
  };
};

export class FireflyClient {
  static readonly PAGE_SIZE = 100;

  private readonly baseUrl: string;
  private readonly token: string;

  constructor(baseUrl: string, token: string) {
    this.baseUrl = baseUrl;
    this.token = token;
  }

  async accounts(kind: AccountKind): Promise<FireflyAccount[]> {
    return this.paginate<FireflyAccount>("/api/v1/accounts", {
      type: kind === "asset" ? "asset" : "liabilities",
    });
  }

  async createAccount(account: NewFireflyAccount): Promise<FireflyAccount> {
    const body = await this.request<{ data: FireflyAccount }>("POST", "/api/v1/accounts", account);

    return body.data;
  }

  async piggyBanks(): Promise<PiggyBank[]> {
    return this.paginate<PiggyBank>("/api/v1/piggy-banks", {});
  }

  async updatePiggyBank(piggyBank: PiggyBank, update: PiggyBankUpdate): Promise<void> {
    await this.request("PUT", `/api/v1/piggy-banks/${piggyBank.id}`, update);
  }

  async existingBetween(start: Date, end: Date): Promise<Map<string, ExistingTransaction>> {
    const groups = await this.paginate<FireflyTransactionGroup>("/api/v1/transactions", {
      type: "all",
      start: FireflyClient.dateOnly(start),
      end: FireflyClient.dateOnly(end),
    });

    const existing = new Map<string, ExistingTransaction>();

    for (const group of groups) {
      for (const transaction of group.attributes.transactions) {
        if (!transaction.external_id) {
          continue;
        }

        existing.set(transaction.external_id, {
          groupId: group.id,
          journalId: String(transaction.transaction_journal_id),
          splits: group.attributes.transactions.length,
          externalId: transaction.external_id,
          type: transaction.type,
          date: transaction.date,
          amount: transaction.amount,
          sourceId: String(transaction.source_id),
          destinationId: String(transaction.destination_id),
          tags: transaction.tags ?? [],
        });
      }
    }

    return existing;
  }

  async updateTransaction(existing: ExistingTransaction, update: TransactionUpdate): Promise<void> {
    await this.request("PUT", `/api/v1/transactions/${existing.groupId}`, {
      apply_rules: false,
      fire_webhooks: true,
      transactions: [{ transaction_journal_id: existing.journalId, ...update }],
    });
  }

  async deleteTransaction(existing: ExistingTransaction): Promise<void> {
    await this.request("DELETE", `/api/v1/transactions/${existing.groupId}`);
  }

  async createTransaction(transaction: FireflyTransaction): Promise<void> {
    await this.request("POST", "/api/v1/transactions", {
      error_if_duplicate_hash: false,
      apply_rules: true,
      fire_webhooks: true,
      transactions: [transaction],
    });
  }

  private static dateOnly(date: Date): string {
    return date.toISOString().slice(0, 10);
  }

  private async paginate<T>(path: string, params: Record<string, string>): Promise<T[]> {
    const items: T[] = [];
    let page = 1;
    let totalPages = 1;

    while (page <= totalPages) {
      const query = new URLSearchParams({
        ...params,
        limit: String(FireflyClient.PAGE_SIZE),
        page: String(page),
      });

      const body = await this.request<FireflyPage<T>>("GET", `${path}?${query}`);

      items.push(...body.data);
      totalPages = body.meta.pagination.total_pages;
      page += 1;
    }

    return items;
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${this.token}`,
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

    if (!response.ok) {
      const pathname = path.split("?")[0];
      throw new Error(`Firefly ${method} ${pathname} failed: ${response.status} ${await response.text()}`);
    }

    if (response.status === 204) {
      return undefined as T;
    }

    return (await response.json()) as T;
  }
}
