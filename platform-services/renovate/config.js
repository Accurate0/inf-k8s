module.exports = {
  platform: "forgejo",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverTopics: ["managed-by-renovate"],
  dependencyDashboard: true,
  gitAuthor: "renovate[bot] <noreply@git.anurag.sh>",
  // Forgejo 404s on /commits/{ref}/statuses when ref contains an encoded slash,
  // which Renovate maps to REPOSITORY_CHANGED and aborts the repo. Drop the slash.
  branchPrefix: "renovate-",
  allowedUnsafeExecutions: ["gradleWrapper"],
  extends: ["config:recommended"],
};
