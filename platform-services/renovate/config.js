module.exports = {
  platform: "forgejo",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverTopics: ["managed-by-renovate"],
  dependencyDashboard: true,
  configMigration: true,
  osvVulnerabilityAlerts: true,
  prConcurrentLimit: 3,
  gitAuthor: "renovate[bot] <noreply@git.anurag.sh>",
  allowedUnsafeExecutions: ["gradleWrapper"],
  allowedCommands: ["^sh -c 'cd eink-display-web && pnpm .*"],
  lockFileMaintenance: {
    enabled: false,
  },
  extends: ["config:recommended"],
};
