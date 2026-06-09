module.exports = {
  platform: "forgejo",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverTopics: ["managed-by-renovate"],
  dependencyDashboard: true,
  gitAuthor: "renovate[bot] <noreply@git.anurag.sh>",
  allowedUnsafeExecutions: ["gradleWrapper"],
  extends: ["config:recommended"],
};
