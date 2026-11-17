import { fs } from '@ortzi/host';
import { formatApp } from '../fmt.ts';

export function fmt(paths: string[], check: boolean): number {
  const unformatted = paths.filter((path) => {
    const text = fs.readText(path);
    const formatted = formatApp(text);
    if (formatted === text) {
      return false;
    }

    if (!check) {
      fs.write(path, formatted);
    }
    return true;
  });

  for (const path of unformatted) {
    console.log(check ? `${path} is not formatted` : `formatted ${path}`);
  }

  return check && unformatted.length ? 1 : 0;
}
