export type Level = 'ok' | 'warn' | 'fail';

export interface Check {
  level: Level;
  text: string;
}

export interface Condition {
  type: string;
  status: string;
  reason?: string;
  message?: string;
}

export interface ParentRef {
  name: string;
  namespace?: string;
  sectionName?: string;
}

export interface KubeObject {
  metadata: { name: string; labels?: Record<string, string> };
  spec?: Record<string, any>;
  status?: Record<string, any>;
}

export const sameParent = (a: ParentRef, b: ParentRef) =>
  a.name === b.name && a.namespace === b.namespace && a.sectionName === b.sectionName;

export const ok = (text: string): Check => ({ level: 'ok', text });
export const warn = (text: string): Check => ({ level: 'warn', text });
export const fail = (text: string): Check => ({ level: 'fail', text });

export const unmet = (conditions: Condition[] | undefined, wanted: string[]) =>
  wanted.flatMap((type) => {
    const found = conditions?.find((c) => c.type === type);
    if (found?.status === 'True') {
      return [];
    }
    return [`${type}${found?.message ? `: ${found.message}` : ' not reported'}`];
  });
