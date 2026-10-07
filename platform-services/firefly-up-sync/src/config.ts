export type CardAccountConfig = {
  name: string;
  match: string;
};

export class Config {
  readonly upToken: string;
  readonly fireflyUrl: string;
  readonly fireflyToken: string;
  readonly lookbackDays: number;
  readonly concurrency: number;
  readonly dryRun: boolean;
  readonly cardAccounts: CardAccountConfig[];

  constructor(env: NodeJS.ProcessEnv) {
    this.upToken = Config.required(env, "UP_TOKEN");
    this.fireflyUrl = Config.required(env, "FIREFLY_URL").replace(/\/+$/, "");
    this.fireflyToken = Config.required(env, "FIREFLY_TOKEN");
    this.lookbackDays = Config.positiveInteger(env, "LOOKBACK_DAYS", 14);
    this.concurrency = Config.positiveInteger(env, "CONCURRENCY", 1);
    this.dryRun = env.DRY_RUN === "true";
    this.cardAccounts = Config.cardAccounts(env, "CARD_ACCOUNTS");
  }

  private static cardAccounts(env: NodeJS.ProcessEnv, name: string): CardAccountConfig[] {
    const raw = env[name]?.trim();

    if (!raw) {
      return [];
    }

    const parsed: unknown = JSON.parse(raw);

    if (!Array.isArray(parsed)) {
      throw new Error(`${name} must be a JSON array`);
    }

    return parsed.map((entry) => {
      const card = entry as Partial<CardAccountConfig>;

      if (typeof card.name !== "string" || !card.name.trim() || typeof card.match !== "string" || !card.match.trim()) {
        throw new Error(`${name} entries need a non-empty "name" and "match"`);
      }

      return { name: card.name.trim(), match: card.match.trim() };
    });
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
