export type UpMoney = {
  currencyCode: string;
  value: string;
  valueInBaseUnits: number;
};

export type UpAccount = {
  id: string;
  attributes: {
    displayName: string;
    accountType: string;
    ownershipType: string;
    balance: UpMoney;
  };
};

export type UpRelationship = {
  data: { id: string } | null;
};

export type UpTransaction = {
  id: string;
  attributes: {
    status: string;
    rawText: string | null;
    description: string;
    message: string | null;
    amount: UpMoney;
    foreignAmount: UpMoney | null;
    createdAt: string;
    settledAt: string | null;
  };
  relationships: {
    account: { data: { id: string } };
    transferAccount: UpRelationship;
    category: UpRelationship;
    tags: { data: { id: string }[] };
  };
};

type UpPage<T> = {
  data: T[];
  links: { next: string | null };
};

export class UpClient {
  static readonly BASE_URL = "https://api.up.com.au/api/v1";
  static readonly PAGE_SIZE = 100;

  private readonly token: string;

  constructor(token: string) {
    this.token = token;
  }

  async accounts(): Promise<UpAccount[]> {
    const url = new URL(`${UpClient.BASE_URL}/accounts`);
    url.searchParams.set("page[size]", String(UpClient.PAGE_SIZE));

    return this.paginate<UpAccount>(url.toString());
  }

  async transactionsSince(since: Date): Promise<UpTransaction[]> {
    const url = new URL(`${UpClient.BASE_URL}/transactions`);
    url.searchParams.set("page[size]", String(UpClient.PAGE_SIZE));
    url.searchParams.set("filter[since]", since.toISOString());

    return this.paginate<UpTransaction>(url.toString());
  }

  private async paginate<T>(firstUrl: string): Promise<T[]> {
    const items: T[] = [];
    let next: string | null = firstUrl;

    while (next) {
      const page: UpPage<T> = await this.get<UpPage<T>>(next);

      items.push(...page.data);
      next = page.links.next;
    }

    return items;
  }

  private async get<T>(url: string): Promise<T> {
    const response = await fetch(url, {
      headers: {
        Authorization: `Bearer ${this.token}`,
        Accept: "application/json",
      },
    });

    if (!response.ok) {
      const path = new URL(url).pathname;
      throw new Error(`Up API GET ${path} failed: ${response.status} ${await response.text()}`);
    }

    return (await response.json()) as T;
  }
}
