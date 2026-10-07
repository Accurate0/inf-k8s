export type LinkedAccountConfig = {
  name: string;
  matches: string[];
  ids: string[];
};

export class Config {
  readonly upToken: string;
  readonly fireflyUrl: string;
  readonly fireflyToken: string;
  readonly lookbackDays: number;
  readonly concurrency: number;
  readonly dryRun: boolean;
  readonly linkedAccounts: LinkedAccountConfig[];
  readonly payoffPiggyBank: string | undefined;

  constructor(env: NodeJS.ProcessEnv) {
    this.payoffPiggyBank = env.PAYOFF_PIGGY_BANK?.trim() || undefined;
    this.upToken = Config.required(env, "UP_TOKEN");
    this.fireflyUrl = Config.required(env, "FIREFLY_URL").replace(/\/+$/, "");
    this.fireflyToken = Config.required(env, "FIREFLY_TOKEN");
    this.lookbackDays = Config.positiveInteger(env, "LOOKBACK_DAYS", 14);
    this.concurrency = Config.positiveInteger(env, "CONCURRENCY", 1);
    this.dryRun = env.DRY_RUN === "true";
    this.linkedAccounts = Config.linkedAccounts(env, "LINKED_ACCOUNTS");
  }

  private static linkedAccounts(env: NodeJS.ProcessEnv, name: string): LinkedAccountConfig[] {
    const raw = env[name]?.trim();

    if (!raw) {
      return [];
    }

    const parsed: unknown = JSON.parse(raw);

    if (!Array.isArray(parsed)) {
      throw new Error(`${name} must be a JSON array`);
    }

    return parsed.map((entry) => {
      const account = entry as { name?: unknown; matches?: unknown; ids?: unknown };
      const matches = Config.strings(account.matches);
      const ids = Config.strings(account.ids);

      if (typeof account.name !== "string" || !account.name.trim()) {
        throw new Error(`${name} entries need a non-empty "name"`);
      }

      if (matches.length === 0 && ids.length === 0) {
        throw new Error(`${name} entry "${account.name}" needs at least one of "matches" or "ids"`);
      }

      return { name: account.name.trim(), matches, ids };
    });
  }

  private static strings(value: unknown): string[] {
    if (value === undefined) {
      return [];
    }

    if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || !item.trim())) {
      throw new Error(`expected an array of non-empty strings, got ${JSON.stringify(value)}`);
    }

    return value.map((item: string) => item.trim());
  }

  private static required(env: NodeJS.ProcessEnv, name: string): string {
    const value = env[name]?.trim();

    if (!value) {
      throw new Error(`${name} must be set`);
    }

    return value;
  }

  private static positiveInteger(env: NodeJS.ProcessEnv, name: string, fallback: number): number {
    const raw = env[name]?.trim();

    if (!raw) {
      return fallback;
    }

    const value = Number(raw);

    if (!Number.isInteger(value) || value <= 0) {
      throw new Error(`${name} must be a positive integer, got "${raw}"`);
    }

    return value;
  }
}
