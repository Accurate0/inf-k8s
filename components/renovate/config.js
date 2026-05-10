module.exports = {
  platform: "forgejo",
  endpoint: "https://git.anurag.sh/api/v1",
  autodiscover: true,
  autodiscoverFilter: "/topic/renovate/",
  dependencyDashboard: true,
  extends: ["config:recommended"],
};
