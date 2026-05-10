module.exports = {
  platform: "gitea",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverFilter: "/topic/renovate/",
  dependencyDashboard: true,
  gitAuthor: "Renovate <noreply@git.anurag.sh>"
  extends: ["config:recommended"],
};
