export const HEADER_TEXT = 'generated from app.yaml by `ortzi app render`; edit app.yaml instead';

export const HEADER = `# ${HEADER_TEXT}`;

const STAMP = /^# generated at .*\n/m;

export const stamped = (rendered: string, now: Date, header = HEADER) =>
  rendered.replace(`${header}\n`, `${header}\n# generated at ${now.toISOString()}\n`);

export const isCurrent = (existing: string, rendered: string) =>
  STAMP.test(existing) && existing.replace(STAMP, '') === rendered;
