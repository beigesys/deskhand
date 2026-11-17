//! Two-step sign-in for password users: TOTP (RFC 6238: SHA-1, 6 digits, 30 seconds) and
//! ten one-time recovery codes. Single sign-on skips it; the identity provider does MFA.
//!
//! A correct password for a user with two-step on doesn't start a session: it returns an
//! `mfa_token` (five minutes, five tries) that `POST /api/v1/login/mfa` trades, with a code,
//! for one. With `DESKHAND_REQUIRE_MFA=1`, password users without it get an enrollment
//! token instead, and set it up before their first session.

use crate::api::{bad, err, me_of, need, ok, signed_in, Api, ApiError, Me};
use crate::audit::Entry;
use crate::auth::{self, client_ip};
use crate::db::{Role, User};
use crate::{now, random_hex, App, Shared};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::Sha1;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const STEP: i64 = 30;
const ISSUER: &str = "Deskhand";
const RECOVERY_CODES: usize = 10;
const CHALLENGE_TTL: Duration = Duration::from_secs(300);
const CHALLENGE_TRIES: u8 = 5;

// --- TOTP ---

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from_be_bytes([0, 0, 0, buf[0], buf[1], buf[2], buf[3], buf[4]]);
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..chars {
            out.push(B32[((bits >> (35 - i * 5)) & 31) as usize] as char);
        }
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut acc, mut n) = (0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let v = B32.iter().position(|b| *b == c.to_ascii_uppercase())? as u32;
        acc = (acc << 5) | v;
        n += 5;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
            acc &= (1 << n) - 1;
        }
    }
    Some(out)
}

/// The 6-digit code for `key` at time step `step`.
pub fn totp(key: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(&(step as u64).to_be_bytes());
    let h = mac.finalize().into_bytes();
    let o = (h[19] & 0xf) as usize;
    (u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]])) % 1_000_000
}

/// The time step `code` is for, allowing one step of clock drift either way.
pub fn check(secret: &str, code: &str, now: i64) -> Option<i64> {
    let code: u32 = code.parse().ok()?;
    let key = base32_decode(secret)?;
    let step = now / STEP;
    [step, step - 1, step + 1].into_iter().find(|s| totp(&key, *s) == code)
}

fn new_secret() -> String {
    let mut b = [0u8; 20];
    getrandom::fill(&mut b).expect("random bytes");
    base32_encode(&b)
}

fn otpauth_uri(secret: &str, username: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20");
    format!(
        "otpauth://totp/{}:{}?secret={secret}&issuer={}&algorithm=SHA1&digits=6&period={STEP}",
        enc(ISSUER),
        enc(username),
        enc(ISSUER)
    )
}

fn qr_svg(data: &str) -> String {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(q) => q
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

// --- recovery codes ---

/// `xxxxx-xxxxx` from lower-case base32: 50 random bits each.
fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut b = [0u8; 10];
            getrandom::fill(&mut b).expect("random bytes");
            let s: String = b.iter().map(|x| (B32[(*x & 31) as usize] as char).to_ascii_lowercase()).collect();
            format!("{}-{}", &s[..5], &s[5..])
        })
        .collect()
}

fn normalize_recovery(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Ten argon2 hashes: slow on purpose, so off the async workers.
fn hash_codes(codes: &[String]) -> Vec<String> {
    tokio::task::block_in_place(|| codes.iter().map(|c| auth::hash_password(&normalize_recovery(c))).collect())
}

/// What a sign-in code turned out to be.
enum Used {
    Totp,
    Recovery { left: i64 },
}

/// Checks a TOTP code (6 digits) or, when `recovery`, a recovery code for `user`, using it
/// up. None if it's wrong, already used, or the user hasn't two-step on.
fn verify(app: &App, user: i64, code: &str, recovery: bool) -> Result<Option<Used>, rusqlite::Error> {
    let Some(state) = app.db.mfa_state(user)? else { return Ok(None) };
    let Some(secret) = state.secret else { return Ok(None) };
    let digits: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(match check(&secret, &digits, now()) {
            Some(step) if app.db.claim_totp_step(user, step)? => Some(Used::Totp),
            _ => None,
        });
    }
    if !recovery {
        return Ok(None);
    }
    let given = normalize_recovery(code);
    if given.len() != 10 {
        return Ok(None);
    }
    let codes = app.db.recovery_codes(user)?;
    let found = tokio::task::block_in_place(|| codes.into_iter().find(|(_, hash)| auth::verify_password(&given, hash)));
    match found {
        Some((id, _)) if app.db.use_recovery_code(id, now())? => Ok(Some(Used::Recovery { left: state.recovery_left - 1 })),
        _ => Ok(None),
    }
}

// --- sign-in challenges ---

pub struct Challenge {
    user: i64,
    expires: Instant,
    tries: u8,
    /// Set up two-step first (DESKHAND_REQUIRE_MFA), with this secret once shown.
    enroll: Option<Option<String>>,
}

#[derive(Default)]
pub struct Challenges(Mutex<HashMap<String, Challenge>>);

impl Challenges {
    fn add(&self, user: i64, enroll: bool) -> String {
        let token = random_hex(24);
        let mut m = self.0.lock().unwrap();
        let now = Instant::now();
        m.retain(|_, c| c.expires > now);
        m.insert(
            token.clone(),
            Challenge { user, expires: now + CHALLENGE_TTL, tries: 0, enroll: enroll.then_some(None) },
        );
        token
    }

    /// Runs `f` on a live challenge of the right kind, counting a try; drops it when it's
    /// out of tries.
    fn with<T>(&self, token: &str, enroll: bool, f: impl FnOnce(&mut Challenge) -> T) -> Result<T, ApiError> {
        let mut m = self.0.lock().unwrap();
        let gone = || err(StatusCode::UNAUTHORIZED, "that sign-in has expired; start again");
        let c = m.get_mut(token).filter(|c| c.expires > Instant::now() && c.enroll.is_some() == enroll).ok_or_else(gone)?;
        c.tries += 1;
        if c.tries > CHALLENGE_TRIES {
            m.remove(token);
            return Err(gone());
        }
        Ok(f(c))
    }

    fn remove(&self, token: &str) {
        self.0.lock().unwrap().remove(token);
    }
}

/// What a correct password leads to.
pub enum Next {
    Session,
    Verify(String),
    Enroll(String),
}

/// After a correct password: straight in, or a challenge for the second step.
pub fn after_password(app: &App, user: &User) -> Result<Next, rusqlite::Error> {
    let state = app.db.mfa_state(user.id)?;
    Ok(match state {
        Some(s) if s.secret.is_some() => Next::Verify(app.mfa.add(user.id, false)),
        Some(s) if app.require_mfa && s.has_password => Next::Enroll(app.mfa.add(user.id, true)),
        _ => Next::Session,
    })
}

/// The API's answer to a correct password.
pub fn password_ok(app: &Shared, headers: &HeaderMap, user: &User, detail: &str) -> Result<Response, ApiError> {
    match after_password(app, user)? {
        Next::Session => signed_in(app, headers, user, "login", detail),
        Next::Verify(t) => Ok(Json(json!({ "mfa_required": true, "mfa_token": t })).into_response()),
        Next::Enroll(t) => Ok(Json(json!({ "mfa_enroll_required": true, "mfa_token": t })).into_response()),
    }
}

// --- API ---

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/login/mfa", post(login_mfa))
        .route("/login/mfa/enroll", post(login_enroll))
        .route("/login/mfa/enroll/confirm", post(login_enroll_confirm))
        .route("/me/mfa", axum::routing::get(status))
        .route("/me/mfa/enroll", post(enroll))
        .route("/me/mfa/confirm", post(confirm))
        .route("/me/mfa/recovery-codes", post(regenerate))
        .route("/me/mfa/disable", post(disable))
        .route("/users/{id}/mfa/reset", post(reset))
}

#[derive(Deserialize)]
struct MfaLogin {
    mfa_token: String,
    #[serde(default)]
    code: String,
}

fn limited(app: &Shared, headers: &HeaderMap, peer: SocketAddr) -> Result<(), ApiError> {
    if app.login_limit.allow(client_ip(app, headers, peer)) {
        Ok(())
    } else {
        Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute"))
    }
}

fn user_by_id(app: &Shared, id: i64) -> Result<User, ApiError> {
    app.db
        .users()?
        .into_iter()
        .map(|(u, _)| u)
        .find(|u| u.id == id)
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "that user is gone"))
}

/// `POST /login/mfa {mfa_token, code}`: the second step, with a TOTP or recovery code.
async fn login_mfa(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<MfaLogin>,
) -> Result<Response, ApiError> {
    limited(&app, &headers, peer)?;
    let uid = app.mfa.with(&b.mfa_token, false, |c| c.user)?;
    let user = user_by_id(&app, uid)?;
    match verify(&app, uid, &b.code, true)? {
        Some(used) => {
            app.mfa.remove(&b.mfa_token);
            let how = match used {
                Used::Totp => "password + authenticator code".to_owned(),
                Used::Recovery { left } => {
                    app.db.record(
                        Entry::new("mfa.recovery_used").by(uid).target("user", uid).data(json!({ "left": left })),
                    );
                    format!("password + recovery code ({left} left)")
                }
            };
            signed_in(&app, &headers, &user, "login", &how)
        }
        None => {
            app.db.record(
                Entry::new("login.failed")
                    .target("user", uid)
                    .detail(format!("{}: wrong two-step code", user.username))
                    .data(json!({ "username": user.username, "reason": "mfa" })),
            );
            Err(err(StatusCode::UNAUTHORIZED, "wrong code"))
        }
    }
}

#[derive(Serialize)]
struct Enrollment {
    secret: String,
    uri: String,
    qr_svg: String,
}

fn enrollment(secret: String, username: &str) -> Json<Enrollment> {
    let uri = otpauth_uri(&secret, username);
    Json(Enrollment { qr_svg: qr_svg(&uri), uri, secret })
}

/// `POST /login/mfa/enroll {mfa_token}`: the secret to set up, when two-step is required.
async fn login_enroll(State(app): State<Shared>, Json(b): Json<MfaLogin>) -> Api<Enrollment> {
    let (uid, secret) = app.mfa.with(&b.mfa_token, true, |c| {
        let secret = c.enroll.as_mut().unwrap().get_or_insert_with(new_secret).clone();
        (c.user, secret)
    })?;
    let user = user_by_id(&app, uid)?;
    Ok(enrollment(secret, &user.username))
}

#[derive(Serialize)]
struct Enrolled {
    me: Me,
    recovery_codes: Vec<String>,
}

/// `POST /login/mfa/enroll/confirm {mfa_token, code}`: turns two-step on and signs in.
async fn login_enroll_confirm(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<MfaLogin>,
) -> Result<Response, ApiError> {
    limited(&app, &headers, peer)?;
    let (uid, secret) = app.mfa.with(&b.mfa_token, true, |c| (c.user, c.enroll.clone().flatten()))?;
    let secret = secret.ok_or_else(|| bad("fetch the secret first"))?;
    let user = user_by_id(&app, uid)?;
    let step = check(&secret, b.code.trim(), now()).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "wrong code"))?;
    let codes = new_recovery_codes();
    app.db.enable_totp(uid, &secret, step, now(), &hash_codes(&codes))?;
    app.mfa.remove(&b.mfa_token);
    app.db.record(Entry::new("mfa.enable").by(uid).target("user", uid).detail("at sign-in (required)"));
    let cookie = auth::new_session(&app, &headers, &user, "login", "password + new authenticator")
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "couldn't start a session"))?;
    Ok(([(header::SET_COOKIE, cookie)], Json(Enrolled { me: me_of(&user), recovery_codes: codes })).into_response())
}

#[derive(Serialize)]
struct Status {
    enabled: bool,
    enabled_at: Option<i64>,
    recovery_left: i64,
    /// Whether the user signs in with a password at all (two-step is for those who do).
    password_user: bool,
    /// The server requires two-step for password users (it can't be turned off).
    required: bool,
}

async fn status(State(app): State<Shared>, headers: HeaderMap) -> Api<Status> {
    let user = need(&app, &headers, Role::Viewer)?;
    let s = app.db.mfa_state(user.id)?.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "sign in first"))?;
    Ok(Json(Status {
        enabled: s.secret.is_some(),
        enabled_at: s.enabled_at,
        recovery_left: s.recovery_left,
        password_user: s.has_password,
        required: app.require_mfa && s.has_password,
    }))
}

/// `POST /me/mfa/enroll`: a new secret to scan, confirmed by `/me/mfa/confirm`.
async fn enroll(State(app): State<Shared>, headers: HeaderMap) -> Api<Enrollment> {
    let user = need(&app, &headers, Role::Viewer)?;
    let s = app.db.mfa_state(user.id)?.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "sign in first"))?;
    if s.secret.is_some() {
        return Err(err(StatusCode::CONFLICT, "two-step sign-in is already on"));
    }
    if !s.has_password {
        return Err(bad("you sign in with single sign-on; your identity provider handles two-step"));
    }
    let secret = new_secret();
    app.db.set_totp_pending(user.id, Some(&secret))?;
    Ok(enrollment(secret, &user.username))
}

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

#[derive(Serialize)]
struct Codes {
    recovery_codes: Vec<String>,
}

async fn confirm(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<CodeBody>) -> Api<Codes> {
    let user = need(&app, &headers, Role::Viewer)?;
    let s = app.db.mfa_state(user.id)?.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "sign in first"))?;
    let secret = s.pending.ok_or_else(|| bad("start setup first"))?;
    let step = check(&secret, b.code.trim(), now()).ok_or_else(|| bad("that code doesn't match; check the time on your phone"))?;
    let codes = new_recovery_codes();
    app.db.enable_totp(user.id, &secret, step, now(), &hash_codes(&codes))?;
    app.db.record(Entry::new("mfa.enable").by(user.id).target("user", user.id));
    Ok(Json(Codes { recovery_codes: codes }))
}

/// New recovery codes (the old ones stop working), for a current authenticator code.
async fn regenerate(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<CodeBody>) -> Api<Codes> {
    let user = need(&app, &headers, Role::Viewer)?;
    if verify(&app, user.id, &b.code, false)?.is_none() {
        return Err(bad("wrong authenticator code"));
    }
    let codes = new_recovery_codes();
    app.db.replace_recovery_codes(user.id, &hash_codes(&codes))?;
    app.db.record(Entry::new("mfa.recovery_regenerate").by(user.id).target("user", user.id));
    Ok(Json(Codes { recovery_codes: codes }))
}

async fn disable(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<CodeBody>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Viewer)?;
    let s = app.db.mfa_state(user.id)?.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "sign in first"))?;
    if app.require_mfa && s.has_password {
        return Err(err(StatusCode::CONFLICT, "this server requires two-step sign-in"));
    }
    if verify(&app, user.id, &b.code, true)?.is_none() {
        return Err(bad("wrong code"));
    }
    app.db.disable_totp(user.id)?;
    app.db.record(Entry::new("mfa.disable").by(user.id).target("user", user.id));
    Ok(ok())
}

/// `POST /users/{id}/mfa/reset` (admin): turns a user's two-step off, for a lost phone.
/// With DESKHAND_REQUIRE_MFA they set it up again at their next sign-in.
async fn reset(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let admin = need(&app, &headers, Role::Admin)?;
    if app.db.disable_totp(id)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such user"));
    }
    app.db.record(Entry::new("mfa.reset").by(admin.id).target("user", id));
    Ok(ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        // RFC 6238 appendix B, SHA-1, last six digits.
        let key = b"12345678901234567890";
        for (t, code) in [(59, 287082), (1111111109, 81804), (1111111111, 50471), (1234567890, 5924), (2000000000, 279037)] {
            assert_eq!(totp(key, t / 30), code, "t={t}");
        }
    }

    #[test]
    fn base32_round_trip() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        for n in 0..30 {
            let v: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(base32_decode(&base32_encode(&v)).unwrap(), v);
        }
        assert_eq!(base32_decode("mzxw6ytboi").unwrap(), b"foobar");
    }

    #[test]
    fn check_allows_one_step_of_drift() {
        let secret = new_secret();
        let key = base32_decode(&secret).unwrap();
        let now = 1_700_000_000;
        for d in [-1, 0, 1] {
            let code = format!("{:06}", totp(&key, now / 30 + d));
            assert_eq!(check(&secret, &code, now), Some(now / 30 + d));
        }
        let code = format!("{:06}", totp(&key, now / 30 + 3));
        assert_eq!(check(&secret, &code, now), None);
    }

    #[test]
    fn recovery_codes_look_right() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 11 && c.as_bytes()[5] == b'-'));
        assert_eq!(normalize_recovery(" ABCDE-fghij "), "abcdefghij");
    }

    #[test]
    fn uri_escapes_user() {
        let u = otpauth_uri("ABC", "a b@c");
        assert!(u.starts_with("otpauth://totp/Deskhand:a%20b%40c?secret=ABC&issuer=Deskhand"), "{u}");
        assert!(qr_svg(&u).contains("<svg"));
    }
}
