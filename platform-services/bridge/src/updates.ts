import type { EventEmitter } from 'node:events';

export interface UpdatesOptions {
  source: EventEmitter;
  debounceMs?: number;
  intervalMs?: number;
}

const MAX_QUEUED = 100;

// rechecks after changes settle and on an interval, pushing whatever check returns; return() unsubscribes
export function updates<T>(
  check: () => Promise<T[]>,
  { source, debounceMs = 1_000, intervalMs }: UpdatesOptions,
): AsyncIterableIterator<T> {
  const queue: T[] = [];
  let waiting: ((result: IteratorResult<T>) => void) | undefined;
  let done = false;
  let debounce: NodeJS.Timeout | undefined;
  let chain = Promise.resolve();

  const push = (value: T) => {
    if (waiting) {
      const resolve = waiting;
      waiting = undefined;
      resolve({ value, done: false });
      return;
    }
    queue.push(value);
    if (queue.length > MAX_QUEUED) {
      queue.shift();
    }
  };

  const run = () => {
    chain = chain.then(async () => {
      if (done) {
        return;
      }
      try {
        for (const value of await check()) {
          push(value);
        }
      } catch (error) {
        console.error(`subscription check: ${(error as Error).message}`);
      }
    });
  };

  const schedule = () => {
    if (!done && !debounce) {
      debounce = setTimeout(() => {
        debounce = undefined;
        run();
      }, debounceMs);
    }
  };

  source.on('change', schedule);
  const interval = intervalMs ? setInterval(schedule, intervalMs) : undefined;
  run();

  const stop = () => {
    done = true;
    source.off('change', schedule);
    clearInterval(interval);
    clearTimeout(debounce);
    waiting?.({ value: undefined, done: true });
    waiting = undefined;
  };

  return {
    next: () => {
      if (queue.length) {
        return Promise.resolve({ value: queue.shift()!, done: false });
      }
      if (done) {
        return Promise.resolve({ value: undefined, done: true });
      }
      return new Promise((resolve) => (waiting = resolve));
    },
    return: async () => {
      stop();
      return { value: undefined, done: true };
    },
    throw: async (error) => {
      stop();
      throw error;
    },
    [Symbol.asyncIterator]() {
      return this;
    },
  };
}

// emits a key's value only when its fingerprint differs from the last one sent
export function changed<T>() {
  const sent = new Map<string, string>();
  return (entries: { key: string; fingerprint: string; value: T }[]): T[] => {
    const fresh: T[] = [];
    for (const { key, fingerprint, value } of entries) {
      if (sent.get(key) !== fingerprint) {
        sent.set(key, fingerprint);
        fresh.push(value);
      }
    }
    return fresh;
  };
}
