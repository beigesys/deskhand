//! Single sign-on: Microsoft Entra ID and Google over OpenID Connect, GitHub over OAuth 2.
//! Each provider is on only when its client id is set. Every flow is authorization code
//! with PKCE, a state and (OIDC) a nonce, kept server-side for ten minutes.
//!
//! A login finds its user by provider and subject first, then by a verified email that
//! matches a user's email (linking the identity on the way). New users are created only
//! with `DESKHAND_SSO_AUTO_PROVISION` and only for a provider restricted to one tenant,
//! Workspace domain or org.

use crate::db::{Db, Role, User};
use base64::Engine;
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PENDING_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Entra,
    Google,
    GitHub,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "entra" => Some(Kind::Entra),
            "google" => Some(Kind::Google),
            "github" => Some(Kind::GitHub),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Kind::Entra => "entra",
            Kind::Google => "google",
            Kind::GitHub => "github",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Entra => "Microsoft",
            Kind::Google => "Google",
            Kind::GitHub => "GitHub",
        }
    }
}

/// One configured provider.
pub struct Provider {
    pub kind: Kind,
    client_id: String,
    client_secret: String,
    redirect: String,
    /// OIDC issuer (Entra, Google).
    issuer: String,
    /// GitHub's OAuth and API endpoints.
    github_web: String,
    github_api: String,
    /// The Entra tenant id, Google Workspace domain or GitHub org logins are limited to.
    restriction: Option<String>,
    /// Discovered on first use and kept.
    metadata: tokio::sync::OnceCell<CoreProviderMetadata>,
}

/// What a provider vouched for, after every check passed.
#[derive(Debug)]
pub struct Identity {
    pub kind: Kind,
    pub subject: String,
    /// Only an address the provider says is verified.
    pub verified_email: Option<String>,
    /// For messages: the email or login the user would recognise.
    pub display: String,
}

struct Pending {
    kind: Kind,
    pkce: String,
    nonce: Option<String>,
    created: Instant,
}

pub struct Sso {
    pub providers: Vec<Provider>,
    pending: Mutex<HashMap<String, Pending>>,
    http: openidconnect::reqwest::Client,
    /// Role for users created on first sign-in; `None` keeps provisioning off.
    pub auto_provision: Option<Role>,
}

#[derive(Debug, PartialEq)]
pub enum SsoError {
    /// Unknown, expired, already used, or for another provider.
    State,
    /// The provider's answer failed a check; the reason is for the log and the user.
    Denied(String),
    /// Talking to the provider failed.
    Provider(String),
}

impl std::fmt::Display for SsoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SsoError::State => write!(f, "this sign-in link expired or was already used; start again"),
            SsoError::Denied(r) => write!(f, "{r}"),
            SsoError::Provider(e) => write!(f, "the identity provider didn't answer as expected: {e}"),
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn http_client() -> openidconnect::reqwest::Client {
    // No redirects: an OAuth endpoint that redirects is a misconfiguration, and
    // following them would open the door to SSRF.
    openidconnect::reqwest::ClientBuilder::new()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

impl Provider {
    pub fn entra(tenant: &str, client_id: &str, client_secret: &str, public_url: &str) -> Provider {
        Provider::oidc(
            Kind::Entra,
            &format!("https://login.microsoftonline.com/{tenant}/v2.0"),
            client_id,
            client_secret,
            public_url,
            Some(tenant.to_ascii_lowercase()),
        )
    }

    pub fn oidc(
        kind: Kind,
        issuer: &str,
        client_id: &str,
        client_secret: &str,
        public_url: &str,
        restriction: Option<String>,
    ) -> Provider {
        Provider {
            kind,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect: format!("{}/auth/{}/callback", public_url.trim_end_matches('/'), kind.key()),
            issuer: issuer.into(),
            github_web: String::new(),
            github_api: String::new(),
            restriction,
            metadata: Default::default(),
        }
    }

    pub fn github(client_id: &str, client_secret: &str, public_url: &str, org: Option<String>) -> Provider {
        Provider {
            kind: Kind::GitHub,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect: format!("{}/auth/github/callback", public_url.trim_end_matches('/')),
            issuer: String::new(),
            github_web: "https://github.com".into(),
            github_api: "https://api.github.com".into(),
            restriction: org,
            metadata: Default::default(),
        }
    }

    pub fn restricted(&self) -> bool {
        self.restriction.is_some()
    }

    async fn metadata(&self, http: &openidconnect::reqwest::Client) -> Result<&CoreProviderMetadata, SsoError> {
        self.metadata
            .get_or_try_init(|| async {
                let issuer = IssuerUrl::new(self.issuer.clone()).map_err(|e| SsoError::Provider(e.to_string()))?;
                CoreProviderMetadata::discover_async(issuer, http)
                    .await
                    .map_err(|e| SsoError::Provider(format!("discovery: {e}")))
            })
            .await
    }

    fn redirect_url(&self) -> RedirectUrl {
        RedirectUrl::new(self.redirect.clone()).expect("redirect URL")
    }
}

impl Sso {
    /// From the environment; providers with missing settings are left off with a warning.
    pub fn from_env() -> Sso {
        let mut providers = Vec::new();
        let public = env("DESKHAND_PUBLIC_URL");
        let wants_any = ["DESKHAND_ENTRA_CLIENT_ID", "DESKHAND_GOOGLE_CLIENT_ID", "DESKHAND_GITHUB_CLIENT_ID"]
            .iter()
            .any(|v| env(v).is_some());
        match (&public, wants_any) {
            (None, true) => log::error!("single sign-on is off: set DESKHAND_PUBLIC_URL (the https:// address users open)"),
            (Some(public), _) => {
                if let Some(id) = env("DESKHAND_ENTRA_CLIENT_ID") {
                    match (env("DESKHAND_ENTRA_TENANT_ID"), env("DESKHAND_ENTRA_CLIENT_SECRET")) {
                        (Some(t), _) if ["common", "organizations", "consumers"].contains(&t.to_ascii_lowercase().as_str()) => {
                            log::error!("Entra sign-in is off: DESKHAND_ENTRA_TENANT_ID must be your tenant's id, not {t:?}")
                        }
                        (Some(t), Some(secret)) => providers.push(Provider::entra(&t, &id, &secret, public)),
                        _ => log::error!("Entra sign-in is off: set DESKHAND_ENTRA_TENANT_ID and DESKHAND_ENTRA_CLIENT_SECRET"),
                    }
                }
                if let Some(id) = env("DESKHAND_GOOGLE_CLIENT_ID") {
                    match env("DESKHAND_GOOGLE_CLIENT_SECRET") {
                        Some(secret) => providers.push(Provider::oidc(
                            Kind::Google,
                            "https://accounts.google.com",
                            &id,
                            &secret,
                            public,
                            env("DESKHAND_GOOGLE_HD").map(|d| d.to_ascii_lowercase()),
                        )),
                        None => log::error!("Google sign-in is off: set DESKHAND_GOOGLE_CLIENT_SECRET"),
                    }
                }
                if let Some(id) = env("DESKHAND_GITHUB_CLIENT_ID") {
                    match env("DESKHAND_GITHUB_CLIENT_SECRET") {
                        Some(secret) => providers.push(Provider::github(&id, &secret, public, env("DESKHAND_GITHUB_ORG"))),
                        None => log::error!("GitHub sign-in is off: set DESKHAND_GITHUB_CLIENT_SECRET"),
                    }
                }
            }
            (None, false) => {}
        }
        let auto_provision = match env("DESKHAND_SSO_AUTO_PROVISION").as_deref() {
            None | Some("0") | Some("off") => None,
            Some("viewer") => Some(Role::Viewer),
            Some("operator") => Some(Role::Operator),
            Some(other) => {
                log::error!("DESKHAND_SSO_AUTO_PROVISION={other:?} isn't viewer or operator; provisioning stays off");
                None
            }
        };
        for p in &providers {
            log::info!("{} sign-in on{}", p.kind.label(), match &p.restriction {
                Some(r) => format!(", limited to {r}"),
                None => String::new(),
            });
        }
        Sso::new(providers, auto_provision)
    }

    pub fn new(providers: Vec<Provider>, auto_provision: Option<Role>) -> Sso {
        Sso { providers, pending: Mutex::default(), http: http_client(), auto_provision }
    }

    pub fn provider(&self, kind: Kind) -> Option<&Provider> {
        self.providers.iter().find(|p| p.kind == kind)
    }

    fn remember(&self, state: String, p: Pending) {
        let mut m = self.pending.lock().unwrap();
        m.retain(|_, v| v.created.elapsed() < PENDING_TTL);
        m.insert(state, p);
    }

    /// Takes the pending sign-in for this state: once only, for its own provider, and
    /// within ten minutes.
    fn take(&self, kind: Kind, state: &str) -> Result<Pending, SsoError> {
        let p = self.pending.lock().unwrap().remove(state).ok_or(SsoError::State)?;
        if p.kind != kind || p.created.elapsed() >= PENDING_TTL {
            return Err(SsoError::State);
        }
        Ok(p)
    }

    /// The provider's sign-in page to send the browser to.
    pub async fn start(&self, kind: Kind) -> Result<String, SsoError> {
        let p = self.provider(kind).ok_or(SsoError::State)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        if kind == Kind::GitHub {
            let client = github_client(p)?;
            let mut req = client
                .authorize_url(CsrfToken::new_random)
                .add_scope(oauth2::Scope::new("read:user".into()))
                .add_scope(oauth2::Scope::new("user:email".into()))
                .set_pkce_challenge(oauth2::PkceCodeChallenge::from_code_verifier_sha256(
                    &oauth2::PkceCodeVerifier::new(verifier.secret().clone()),
                ));
            if p.restriction.is_some() {
                req = req.add_scope(oauth2::Scope::new("read:org".into()));
            }
            let (url, state) = req.url();
            self.remember(
                state.secret().clone(),
                Pending { kind, pkce: verifier.secret().clone(), nonce: None, created: Instant::now() },
            );
            return Ok(url.to_string());
        }
        let meta = p.metadata(&self.http).await?;
        let client = CoreClient::from_provider_metadata(
            meta.clone(),
            ClientId::new(p.client_id.clone()),
            Some(ClientSecret::new(p.client_secret.clone())),
        )
        .set_redirect_uri(p.redirect_url());
        let (url, state, nonce) = client
            .authorize_url(
                AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".into()))
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        self.remember(
            state.secret().clone(),
            Pending {
                kind,
                pkce: verifier.secret().clone(),
                nonce: Some(nonce.secret().clone()),
                created: Instant::now(),
            },
        );
        Ok(url.to_string())
    }

    /// Finishes a sign-in from the provider's callback.
    pub async fn finish(&self, kind: Kind, code: &str, state: &str) -> Result<Identity, SsoError> {
        let pending = self.take(kind, state)?;
        let p = self.provider(kind).ok_or(SsoError::State)?;
        if kind == Kind::GitHub {
            return self.finish_github(p, code, pending).await;
        }
        let meta = p.metadata(&self.http).await?;
        let client = CoreClient::from_provider_metadata(
            meta.clone(),
            ClientId::new(p.client_id.clone()),
            Some(ClientSecret::new(p.client_secret.clone())),
        )
        .set_redirect_uri(p.redirect_url());
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|e| SsoError::Provider(e.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce))
            .request_async(&self.http)
            .await
            .map_err(|e| SsoError::Provider(format!("token exchange: {e}")))?;
        let id_token = token
            .id_token()
            .ok_or_else(|| SsoError::Provider("no ID token in the response".into()))?;
        let nonce = Nonce::new(pending.nonce.unwrap_or_default());
        // Signature (JWKS), issuer, audience, expiry and nonce.
        let claims = id_token
            .claims(&client.id_token_verifier(), &nonce)
            .map_err(|e| SsoError::Denied(format!("ID token rejected: {e}")))?;
        // The claims the core types don't carry (tid, hd, xms_edov), from the same,
        // now verified, token.
        let extra = jwt_payload(&id_token.to_string())?;
        let email = claims.email().map(|e| e.as_str().to_owned());
        check_oidc(
            kind,
            p.restriction.as_deref(),
            &extra,
            claims.subject().as_str(),
            email,
            claims.email_verified(),
        )
    }

    async fn finish_github(&self, p: &Provider, code: &str, pending: Pending) -> Result<Identity, SsoError> {
        let client = github_client(p)?;
        let token = client
            .exchange_code(oauth2::AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(oauth2::PkceCodeVerifier::new(pending.pkce))
            .request_async(&self.http)
            .await
            .map_err(|e| SsoError::Provider(format!("token exchange: {e}")))?;
        use oauth2::TokenResponse as _;
        let access = token.access_token().secret();
        let get = |path: String| {
            let req = self
                .http
                .get(format!("{}{path}", p.github_api))
                .bearer_auth(access)
                .header("accept", "application/vnd.github+json")
                .header("user-agent", "deskhand");
            async move {
                let r = req.send().await.map_err(|e| SsoError::Provider(e.to_string()))?;
                let status = r.status();
                let body = r.text().await.map_err(|e| SsoError::Provider(e.to_string()))?;
                Ok::<_, SsoError>((status.as_u16(), body))
            }
        };
        let (status, body) = get("/user".into()).await?;
        if status != 200 {
            return Err(SsoError::Provider(format!("GitHub /user answered {status}")));
        }
        let user: Value = serde_json::from_str(&body).map_err(|e| SsoError::Provider(e.to_string()))?;
        let (status, body) = get("/user/emails".into()).await?;
        let emails: Value = if status == 200 {
            serde_json::from_str(&body).map_err(|e| SsoError::Provider(e.to_string()))?
        } else {
            Value::Null
        };
        let membership = match &p.restriction {
            Some(org) => {
                let (status, body) = get(format!("/user/memberships/orgs/{org}")).await?;
                Some((status, serde_json::from_str(&body).unwrap_or(Value::Null)))
            }
            None => None,
        };
        check_github(p.restriction.as_deref(), &user, &emails, membership.as_ref().map(|(s, v)| (*s, v)))
    }
}

fn github_client(
    p: &Provider,
) -> Result<
    oauth2::basic::BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
    SsoError,
> {
    let bad = |e: oauth2::url::ParseError| SsoError::Provider(e.to_string());
    Ok(oauth2::basic::BasicClient::new(oauth2::ClientId::new(p.client_id.clone()))
        .set_client_secret(oauth2::ClientSecret::new(p.client_secret.clone()))
        .set_auth_uri(oauth2::AuthUrl::new(format!("{}/login/oauth/authorize", p.github_web)).map_err(bad)?)
        .set_token_uri(oauth2::TokenUrl::new(format!("{}/login/oauth/access_token", p.github_web)).map_err(bad)?)
        .set_redirect_uri(oauth2::RedirectUrl::new(p.redirect.clone()).map_err(bad)?))
}

/// A JWT's payload as JSON. Only for a token whose signature was already checked.
fn jwt_payload(jwt: &str) -> Result<Value, SsoError> {
    let part = jwt.split('.').nth(1).ok_or_else(|| SsoError::Provider("malformed ID token".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .map_err(|e| SsoError::Provider(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| SsoError::Provider(e.to_string()))
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// The per-provider checks on a verified ID token's claims.
fn check_oidc(
    kind: Kind,
    restriction: Option<&str>,
    extra: &Value,
    subject: &str,
    email: Option<String>,
    email_verified: Option<bool>,
) -> Result<Identity, SsoError> {
    let verified = match kind {
        Kind::Entra => {
            // Only the configured tenant. (The issuer check already pins the tenant's
            // issuer; tid is checked too, as Microsoft advises.)
            let tid = extra.get("tid").and_then(Value::as_str).unwrap_or("");
            let want = restriction.unwrap_or("");
            if want.is_empty() || !tid.eq_ignore_ascii_case(want) {
                return Err(SsoError::Denied(format!("this Microsoft account belongs to another tenant ({tid})")));
            }
            // Entra doesn't verify the email claim; xms_edov (an optional claim) says the
            // domain's owner did.
            truthy(extra.get("xms_edov")) || email_verified == Some(true)
        }
        Kind::Google => {
            if let Some(hd) = restriction {
                let got = extra.get("hd").and_then(Value::as_str).unwrap_or("");
                if !got.eq_ignore_ascii_case(hd) {
                    let shown = if got.is_empty() { "a personal account" } else { got };
                    return Err(SsoError::Denied(format!("only {hd} Google accounts can sign in, not {shown}")));
                }
            }
            email_verified == Some(true)
        }
        Kind::GitHub => unreachable!("GitHub isn't OIDC"),
    };
    let display = email.clone().unwrap_or_else(|| subject.to_owned());
    Ok(Identity {
        kind,
        subject: subject.to_owned(),
        verified_email: email.filter(|_| verified).map(|e| e.to_ascii_lowercase()),
        display,
    })
}

/// GitHub's checks: org membership when restricted, and the primary verified email.
fn check_github(
    org: Option<&str>,
    user: &Value,
    emails: &Value,
    membership: Option<(u16, &Value)>,
) -> Result<Identity, SsoError> {
    let id = user
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| SsoError::Provider("GitHub user without an id".into()))?;
    let login = user.get("login").and_then(Value::as_str).unwrap_or("").to_owned();
    if let Some(org) = org {
        let active = matches!(membership, Some((200, m)) if m.get("state").and_then(Value::as_str) == Some("active"));
        if !active {
            return Err(SsoError::Denied(format!("{login} isn't an active member of the {org} GitHub org")));
        }
    }
    let primary = emails.as_array().and_then(|a| {
        a.iter().find(|e| truthy(e.get("primary")) && truthy(e.get("verified")))
            .and_then(|e| e.get("email").and_then(Value::as_str))
    });
    Ok(Identity {
        kind: Kind::GitHub,
        subject: id.to_string(),
        verified_email: primary.map(|e| e.to_ascii_lowercase()),
        display: if login.is_empty() { id.to_string() } else { login },
    })
}

/// How a sign-in ended up with its user.
#[derive(Debug, PartialEq)]
pub enum Resolved {
    Known,
    Linked,
    Provisioned,
}

/// Finds or makes the user for a verified identity. `Ok(None)`: nobody matches and
/// provisioning is off (or not allowed for this provider).
pub fn resolve(
    db: &Db,
    sso: &Sso,
    id: &Identity,
    now: i64,
) -> rusqlite::Result<Option<(User, Resolved)>> {
    let provider = id.kind.key();
    if let Some(u) = db.identity_user(provider, &id.subject)? {
        db.link_identity(u.id, provider, &id.subject, id.verified_email.as_deref(), now)?;
        return Ok(Some((u, Resolved::Known)));
    }
    if let Some(email) = &id.verified_email {
        if let Some(u) = db.user_by_email(email)? {
            db.link_identity(u.id, provider, &id.subject, Some(email), now)?;
            return Ok(Some((u, Resolved::Linked)));
        }
    }
    let restricted = sso.provider(id.kind).is_some_and(Provider::restricted);
    let (Some(role), true) = (sso.auto_provision, restricted) else {
        return Ok(None);
    };
    // A user name from the email or login, made unique.
    let base = id.verified_email.clone().unwrap_or_else(|| format!("{}@{provider}", id.display));
    let mut name = base.clone();
    let mut n = 2;
    while db.username_taken(&name)? {
        name = format!("{base}-{n}");
        n += 1;
    }
    // No password: "!" isn't a valid hash, so password sign-in can never match.
    let uid = db.add_user(&name, "!", role, now)?;
    if let Some(email) = &id.verified_email {
        if db.user_by_email(email)?.is_none() {
            db.set_email(uid, Some(email))?;
        }
    }
    db.link_identity(uid, provider, &id.subject, id.verified_email.as_deref(), now)?;
    Ok(Some((User { id: uid, username: name, role }, Resolved::Provisioned)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entra(extra: Value, verified: Option<bool>) -> Result<Identity, SsoError> {
        check_oidc(Kind::Entra, Some("tenant-a"), &extra, "sub1", Some("A@x.com".into()), verified)
    }

    #[test]
    fn entra_wrong_tenant_is_denied() {
        assert!(matches!(entra(json!({"tid": "tenant-b"}), None), Err(SsoError::Denied(_))));
        assert!(matches!(entra(json!({}), None), Err(SsoError::Denied(_))));
        let ok = entra(json!({"tid": "TENANT-A"}), None).unwrap();
        // Entra's email isn't verified without xms_edov.
        assert_eq!(ok.verified_email, None);
        let ok = entra(json!({"tid": "tenant-a", "xms_edov": true}), None).unwrap();
        assert_eq!(ok.verified_email.as_deref(), Some("a@x.com"));
    }

    #[test]
    fn google_wrong_hd_is_denied() {
        let g = |extra: Value| check_oidc(Kind::Google, Some("example.com"), &extra, "s", Some("u@example.com".into()), Some(true));
        assert!(matches!(g(json!({"hd": "evil.com"})), Err(SsoError::Denied(_))));
        // A personal account has no hd, even with a matching-looking email.
        assert!(matches!(g(json!({})), Err(SsoError::Denied(_))));
        assert!(g(json!({"hd": "example.com"})).is_ok());
    }

    #[test]
    fn unverified_email_is_dropped() {
        let id = check_oidc(Kind::Google, None, &json!({}), "s", Some("u@gmail.com".into()), Some(false)).unwrap();
        assert_eq!(id.verified_email, None);
        let id = check_oidc(Kind::Google, None, &json!({}), "s", Some("u@gmail.com".into()), None).unwrap();
        assert_eq!(id.verified_email, None);
    }

    #[test]
    fn github_needs_org_and_primary_verified_email() {
        let user = json!({"id": 42, "login": "octo"});
        let emails = json!([
            {"email": "other@x.com", "primary": false, "verified": true},
            {"email": "Octo@x.com", "primary": true, "verified": true},
        ]);
        let id = check_github(None, &user, &emails, None).unwrap();
        assert_eq!(id.subject, "42");
        assert_eq!(id.verified_email.as_deref(), Some("octo@x.com"));
        let unverified = json!([{"email": "octo@x.com", "primary": true, "verified": false}]);
        assert_eq!(check_github(None, &user, &unverified, None).unwrap().verified_email, None);
        let pending = json!({"state": "pending"});
        assert!(matches!(check_github(Some("acme"), &user, &emails, Some((200, &pending))), Err(SsoError::Denied(_))));
        assert!(matches!(check_github(Some("acme"), &user, &emails, Some((404, &Value::Null))), Err(SsoError::Denied(_))));
        let active = json!({"state": "active"});
        assert!(check_github(Some("acme"), &user, &emails, Some((200, &active))).is_ok());
    }

    #[test]
    fn state_is_single_use_and_bound_to_its_provider() {
        let sso = Sso::new(vec![], None);
        let p = || Pending { kind: Kind::Google, pkce: "v".into(), nonce: None, created: Instant::now() };
        sso.remember("s1".into(), p());
        assert_eq!(sso.take(Kind::GitHub, "s1").err(), Some(SsoError::State));
        // A mismatch still spends it.
        assert_eq!(sso.take(Kind::Google, "s1").err(), Some(SsoError::State));
        sso.remember("s2".into(), p());
        assert_eq!(sso.take(Kind::Google, "nope").err(), Some(SsoError::State));
        assert!(sso.take(Kind::Google, "s2").is_ok());
        assert_eq!(sso.take(Kind::Google, "s2").err(), Some(SsoError::State));
        let mut old = p();
        old.created = Instant::now() - PENDING_TTL;
        sso.remember("s3".into(), old);
        assert_eq!(sso.take(Kind::Google, "s3").err(), Some(SsoError::State));
    }

    /// A fake OIDC provider (discovery, JWKS, token) and one full Google-style sign-in.
    mod fake_provider {
        use super::*;
        use axum::extract::State;
        use axum::routing::{get, post};
        use axum::{Form, Json, Router};
        use openidconnect::core::{
            CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
            CoreRsaPrivateSigningKey,
        };
        use openidconnect::{
            AdditionalClaims, Audience, EndUserEmail, IdToken, IdTokenClaims, JsonWebKeyId,
            PrivateSigningKey, StandardClaims, SubjectIdentifier,
        };
        use serde::{Deserialize, Serialize};
        use sha2::{Digest, Sha256};
        use std::sync::Arc;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct Hd {
            #[serde(skip_serializing_if = "Option::is_none")]
            hd: Option<String>,
        }
        impl AdditionalClaims for Hd {}

        #[derive(Default)]
        struct Fake {
            issuer: String,
            nonce: String,
            challenge: String,
            hd: Option<String>,
        }

        type S = Arc<(Mutex<Fake>, CoreRsaPrivateSigningKey)>;

        fn key() -> CoreRsaPrivateSigningKey {
            CoreRsaPrivateSigningKey::from_pem(include_str!("sso_test_key.pem"), Some(JsonWebKeyId::new("k1".into())))
                .unwrap()
        }

        async fn discovery(State(s): State<S>) -> Json<Value> {
            let iss = s.0.lock().unwrap().issuer.clone();
            Json(json!({
                "issuer": iss,
                "authorization_endpoint": format!("{iss}/authorize"),
                "token_endpoint": format!("{iss}/token"),
                "jwks_uri": format!("{iss}/jwks"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"],
            }))
        }

        async fn jwks(State(s): State<S>) -> Json<CoreJsonWebKeySet> {
            Json(CoreJsonWebKeySet::new(vec![s.1.as_verification_key()]))
        }

        async fn token(State(s): State<S>, Form(f): Form<HashMap<String, String>>) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
            let fake = s.0.lock().unwrap();
            let bad = |m: &str| Err((axum::http::StatusCode::BAD_REQUEST, m.to_owned()));
            if f.get("code").map(String::as_str) != Some("good-code") {
                return bad("bad code");
            }
            // PKCE: the verifier must hash to the challenge sent at the start.
            let verifier = f.get("code_verifier").cloned().unwrap_or_default();
            let hashed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            if hashed != fake.challenge {
                return bad("pkce mismatch");
            }
            let now = chrono_now();
            let claims = IdTokenClaims::new(
                IssuerUrl::new(fake.issuer.clone()).unwrap(),
                vec![Audience::new("cid".into())],
                now + chrono::Duration::seconds(300),
                now,
                StandardClaims::new(SubjectIdentifier::new("google-sub-1".into()))
                    .set_email(Some(EndUserEmail::new("Pat@Example.com".into())))
                    .set_email_verified(Some(true)),
                Hd { hd: fake.hd.clone() },
            )
            .set_nonce(Some(Nonce::new(fake.nonce.clone())));
            let jwt = IdToken::<Hd, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>::new(
                claims,
                &s.1,
                CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
                None,
                None,
            )
            .unwrap();
            Ok(Json(json!({
                "access_token": "at",
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": jwt.to_string(),
            })))
        }

        fn chrono_now() -> chrono::DateTime<chrono::Utc> {
            chrono::Utc::now()
        }

        async fn serve() -> (String, S) {
            let s: S = Arc::new((Mutex::default(), key()));
            let app = Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/jwks", get(jwks))
                .route("/token", post(token))
                .with_state(s.clone());
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", l.local_addr().unwrap());
            s.0.lock().unwrap().issuer = url.clone();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            (url, s)
        }

        /// Runs start, the "browser" leg (copying state, nonce and challenge out of the
        /// URL into the fake), then finish.
        async fn sign_in(sso: &Sso, fake: &S, hd: Option<&str>) -> Result<Identity, SsoError> {
            let url = url::Url::parse(&sso.start(Kind::Google).await.unwrap()).unwrap();
            let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["redirect_uri"], "https://deskhand.test/auth/google/callback");
            {
                let mut f = fake.0.lock().unwrap();
                f.nonce = q["nonce"].clone();
                f.challenge = q["code_challenge"].clone();
                f.hd = hd.map(Into::into);
            }
            sso.finish(Kind::Google, "good-code", &q["state"]).await
        }

        #[tokio::test]
        async fn full_google_style_sign_in() {
            let (issuer, fake) = serve().await;
            let provider = Provider::oidc(Kind::Google, &issuer, "cid", "secret", "https://deskhand.test", Some("example.com".into()));
            let sso = Sso::new(vec![provider], None);

            let id = sign_in(&sso, &fake, Some("example.com")).await.unwrap();
            assert_eq!(id.subject, "google-sub-1");
            assert_eq!(id.verified_email.as_deref(), Some("pat@example.com"));

            // Wrong Workspace domain: the token verifies, the hd check doesn't.
            assert!(matches!(sign_in(&sso, &fake, Some("other.com")).await, Err(SsoError::Denied(_))));

            // A tampered nonce fails the ID token check.
            let url = url::Url::parse(&sso.start(Kind::Google).await.unwrap()).unwrap();
            let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
            {
                let mut f = fake.0.lock().unwrap();
                f.nonce = "not-the-nonce".into();
                f.challenge = q["code_challenge"].clone();
                f.hd = Some("example.com".into());
            }
            assert!(matches!(sso.finish(Kind::Google, "good-code", &q["state"]).await, Err(SsoError::Denied(_))));

            // Linking: nobody matches, then a user with that email gets linked, then
            // the identity alone finds them.
            let db = Db::open(":memory:").unwrap();
            assert!(resolve(&db, &sso, &id, 1).unwrap().is_none());
            let uid = db.add_user("pat", "!", Role::Operator, 1).unwrap();
            db.set_email(uid, Some("PAT@example.com")).unwrap();
            let (u, how) = resolve(&db, &sso, &id, 2).unwrap().unwrap();
            assert_eq!((u.id, how), (uid, Resolved::Linked));
            db.set_email(uid, None).unwrap();
            let (u, how) = resolve(&db, &sso, &id, 3).unwrap().unwrap();
            assert_eq!((u.id, how), (uid, Resolved::Known));
        }

        #[tokio::test]
        async fn provisioning_needs_a_restriction() {
            let db = Db::open(":memory:").unwrap();
            let id = Identity { kind: Kind::Google, subject: "s".into(), verified_email: Some("n@example.com".into()), display: "n@example.com".into() };
            let open = Sso::new(vec![Provider::oidc(Kind::Google, "https://x", "c", "s", "https://i", None)], Some(Role::Viewer));
            assert!(resolve(&db, &open, &id, 1).unwrap().is_none());
            let limited = Sso::new(vec![Provider::oidc(Kind::Google, "https://x", "c", "s", "https://i", Some("example.com".into()))], Some(Role::Viewer));
            let (u, how) = resolve(&db, &limited, &id, 1).unwrap().unwrap();
            assert_eq!((u.username.as_str(), u.role, how), ("n@example.com", Role::Viewer, Resolved::Provisioned));
        }
    }
}
