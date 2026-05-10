module.exports = {
  platform: "forgejo",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverTopics: ["managed-by-renovate"],
  dependencyDashboard: true,
  gitAuthor: "Renovate <noreply@git.anurag.sh>",
  extends: ["config:recommended"],
};
