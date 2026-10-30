import { type Output, type RunOptions, capture, run } from '@ortzi/host';

export class Kube {
  private constructor(
    readonly context: string,
    private readonly scope: string[] = [],
  ) {}

  static connect(context?: string): Kube {
    const resolved = context ?? capture('kubectl', ['config', 'current-context']).trim();
    console.error(`context ${resolved}`);
    return new Kube(resolved);
  }

  namespace(name: string): Kube {
    return new Kube(this.context, ['-n', name]);
  }

  private args(rest: string[]): string[] {
    return ['--context', this.context, ...this.scope, ...rest];
  }

  run(rest: string[], options: RunOptions = {}): Output {
    return run('kubectl', this.args(rest), options);
  }

  capture(rest: string[], options: RunOptions = {}): string {
    return capture('kubectl', this.args(rest), options);
  }

  get<T>(...rest: string[]): T {
    return JSON.parse(this.capture(['get', ...rest, '-o', 'json'])) as T;
  }

  list<T>(...rest: string[]): T[] {
    return this.get<{ items: T[] }>(...rest).items;
  }
}
