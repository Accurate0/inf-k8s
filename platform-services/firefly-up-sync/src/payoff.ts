import type { PiggyBank, PiggyBankUpdate } from "./firefly.ts";
import type { TrackedAccount } from "./planner.ts";
import type { UpAccount } from "./up.ts";

export class Payoff {
  readonly owedCents: number;
  readonly update: PiggyBankUpdate | undefined;

  constructor(piggyBank: PiggyBank, upAccounts: UpAccount[], tracked: Map<string, TrackedAccount>) {
    const balances = new Map<string, number>();
    let owed = 0;

    for (const account of upAccounts) {
      const trackedAccount = tracked.get(account.id);

      if (!trackedAccount) {
        continue;
      }

      const cents = account.attributes.balance.valueInBaseUnits;

      if (trackedAccount.kind === "liability") {
        owed += Math.max(-cents, 0);
        continue;
      }

      balances.set(trackedAccount.id, cents);
    }

    this.owedCents = owed;

    let remaining = owed;

    const accounts = piggyBank.attributes.accounts.map((entry) => {
      const available = Math.max(balances.get(String(entry.account_id)) ?? 0, 0);
      const saved = Math.min(available, remaining);

      remaining -= saved;

      return { account_id: String(entry.account_id), current_amount: Payoff.format(saved) };
    });

    const next = { target_amount: Payoff.format(owed), accounts };

    this.update = Payoff.differs(piggyBank, next) ? next : undefined;
  }

  private static format(cents: number): string {
    return (cents / 100).toFixed(2);
  }

  private static differs(piggyBank: PiggyBank, next: PiggyBankUpdate): boolean {
    if (Number(piggyBank.attributes.target_amount) !== Number(next.target_amount)) {
      return true;
    }

    return piggyBank.attributes.accounts.some(
      (entry, index) => Number(entry.current_amount) !== Number(next.accounts[index]?.current_amount),
    );
  }
}
