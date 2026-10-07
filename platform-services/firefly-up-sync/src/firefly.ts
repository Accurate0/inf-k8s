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
      account_number: string;
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

type FireflyTransactionGroup = {
  attributes: {
    transactions: { external_id: string | null }[];
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

  async externalIdsBetween(start: Date, end: Date): Promise<Set<string>> {
    const groups = await this.paginate<FireflyTransactionGroup>("/api/v1/transactions", {
      type: "all",
      start: FireflyClient.dateOnly(start),
      end: FireflyClient.dateOnly(end),
    });

    const ids = new Set<string>();

    for (const group of groups) {
      for (const transaction of group.attributes.transactions) {
        if (transaction.external_id) {
          ids.add(transaction.external_id);
        }
      }
    }

    return ids;
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

    return (await response.json()) as T;
  }
}
