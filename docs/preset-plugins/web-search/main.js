(async function () {
  const input = JSON.parse(stdin.readAll());
  const query = input.query.trim();
  const limit = input.limit === undefined ? 10 : input.limit;
  if (!query || Array.from(query).length > 600 || query.split(/\s+/u).length > 75) {
    host.fail("SEARCH_INPUT_INVALID", "query exceeds the shared provider limits");
  }
  const bindings = host.httpBindings();
  if (bindings.length === 0) host.fail("SEARCH_NO_PROVIDER", "no enabled search provider");
  const attempts = [];
  let external = 0;
  function clipped(value, maximum) {
    if (typeof value !== "string") host.fail("SEARCH_RESPONSE_INVALID", "invalid provider text");
    return Array.from(value).slice(0, maximum).join("");
  }
  for (let index = 0; index < bindings.length && index < 3; index++) {
    const alias = bindings[index].alias;
    const payload = alias === "brave" ? {q: query, count: limit} : {
      query: query, max_results: limit, search_depth: "basic", topic: "general",
      auto_parameters: false, include_answer: false, include_raw_content: false,
      include_images: false, include_usage: true
    };
    const response = await host.httpRequest(alias, payload, index + 1);
    if (response.outcome === "unknown") throw new Error("upstream outcome unknown");
    if (response.outcome === "not_sent") {
      attempts.push({provider: alias, outcome: "not_sent", http_status: null});
      continue;
    }
    external++;
    const status = response.status_code;
    if (status >= 200 && status < 300) {
      const body = response.body_json;
      if (response.error_code || body === null || typeof body !== "object") {
        host.fail("SEARCH_RESPONSE_INVALID", "invalid provider JSON");
      }
      const results = alias === "brave" ? (body.web ? body.web.results : []) : body.results;
      if (!Array.isArray(results) || (alias === "brave" && !body.query)) {
        host.fail("SEARCH_RESPONSE_INVALID", "invalid provider search shape");
      }
      const items = results.slice(0, limit).map(function (item) {
        if (typeof item.url !== "string" || !/^https?:\/\/[^\s@/?#]+(?:[/?#]|$)/iu.test(item.url) || Array.from(item.url).length > 2048) {
          host.fail("SEARCH_RESPONSE_INVALID", "invalid result URL");
        }
        return {title: clipped(item.title === undefined ? "" : item.title, 1024), url: item.url,
          description: clipped((alias === "brave" ? item.description : item.content) || "", 8192)};
      });
      attempts.push({provider: alias, outcome: "success", http_status: status});
      stdout.write(JSON.stringify({query: query, provider: alias, items: items, count: items.length,
        fallback_used: index > 0, attempts: attempts,
        usage: {external_requests: external, potentially_billed_requests: external}}));
      return;
    }
    const outcome = status === 429 ? "rate_limited" : (status === 401 || status === 403 ? "auth_rejected" : "upstream_error");
    attempts.push({provider: alias, outcome: outcome, http_status: status});
    if (status !== 429 && status !== 401 && status !== 403 && !(status >= 500 && status <= 599)) {
      host.fail("SEARCH_PROVIDERS_EXHAUSTED", "provider returned a non-fallback error");
    }
  }
  host.fail("SEARCH_PROVIDERS_EXHAUSTED", "all configured search providers failed");
})()
