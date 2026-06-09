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
  lockFileMaintenance: {
    enabled: true,
    schedule: ["before 4am on monday"],
  },
  extends: ["config:recommended"],
};
