export const pathspecs = (paths: string[]) =>
  (paths.length ? paths : ['.']).map((path) =>
    path.startsWith('!') ? `:(exclude,glob)${path.slice(1)}` : `:(glob)${path}`,
  );

export const releaseTag = (count: number, sha: string) => `v1.0.${count}-${sha}`;
