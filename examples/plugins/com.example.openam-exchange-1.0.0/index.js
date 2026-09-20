async function access(ctx) {
  const source = ctx.credentials.handles.am;
  const fingerprint = ctx.credentials.fingerprints.am;
  if (!source || !fingerprint) {
    return {outcome: "deny", status: 401, code: "missing_credential"};
  }

  const scopes = [...(ctx.config.scopes || [])].sort();
  const cacheKey = [
    fingerprint,
    ctx.config.translator_url,
    ctx.config.resource,
    scopes.join(" ")
  ].join("|");

  const exchanged = await ctx.cache.getOrLoad(
    cacheKey,
    {ttl_seconds: ctx.config.cache_ttl_seconds, tags: [fingerprint]},
    () => ctx.identity.exchange({
      url: ctx.config.translator_url,
      credential: source,
      credential_field: "subject_token",
      body: {
        grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
        subject_token_type: "urn:ietf:params:oauth:token-type:access_token",
        resource: ctx.config.resource,
        scope: scopes.join(" ")
      },
      token_field: "access_token",
      expires_in_field: "expires_in",
      metadata_fields: ["token_type", "scope"]
    })
  );

  const verification = await ctx.jwt.verify(exchanged.credential, ctx.config.verifier);
  if (!verification.valid) {
    return {outcome: "deny", status: 401, code: "invalid_credential"};
  }
  return {
    outcome: "allow",
    upstream: {bearer: exchanged.credential}
  };
}

function ingress() {
  return {outcome: "allow"};
}

function upstream() {
  return {outcome: "allow"};
}

function response() {
  return {
    outcome: "allow",
    response: {headers: {"x-authenticated-by": "bifrost-plugin"}}
  };
}

function log(ctx) {
  ctx.audit.emit({
    event: "orders_access",
    method: ctx.request.method,
    path: ctx.request.path,
    status: ctx.response.status
  });
  return {outcome: "allow"};
}

module.exports.ingress = ingress;
module.exports.access = access;
module.exports.upstream = upstream;
module.exports.response = response;
module.exports.log = log;
