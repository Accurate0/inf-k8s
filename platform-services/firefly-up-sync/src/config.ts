export class Config {
  readonly upToken: string;
  readonly fireflyUrl: string;
  readonly fireflyToken: string;
  readonly lookbackDays: number;
  readonly dryRun: boolean;

  constructor(env: NodeJS.ProcessEnv) {
    this.upToken = Config.required(env, "UP_TOKEN");
    this.fireflyUrl = Config.required(env, "FIREFLY_URL").replace(/\/+$/, "");
    this.fireflyToken = Config.required(env, "FIREFLY_TOKEN");
    this.lookbackDays = Config.positiveInteger(env, "LOOKBACK_DAYS", 14);
    this.dryRun = env.DRY_RUN === "true";
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
