function envoy_on_request(request_handle)
  local user_agent = string.lower(request_handle:headers():get("user-agent") or "")
  local blocked_patterns = {
    "meta-externalagent",
    "meta-webindexer",
    "gptbot",
    "ccbot",
    "google-extended",
    "bytespider",
    "amazonbot",
    "applebot-extended"
  }

  for _, pattern in ipairs(blocked_patterns) do
    if string.find(user_agent, pattern, 1, true) then
      request_handle:respond({[":status"] = "200"}, "")
      return
    end
  end
end
