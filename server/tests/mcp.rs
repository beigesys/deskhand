//! Runs the server binary and checks MCP end to end: discovery, OAuth 2.1 with dynamic
//! registration and PKCE, refresh rotation, API tokens, and tools acting as their person.

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::process::{Child, Command};
use std::time::Duration;

struct Server {
    child: Child,
    base: String,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start() -> Server {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("deskhand-mcp-test-{port}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_deskhand-server"))
        .env("DESKHAND_ADDR", format!("127.0.0.1:{port}"))
        .env("DESKHAND_DB", dir.join("t.db"))
        .env("DESKHAND_ADMIN_PASSWORD", "admin-password")
        .env("DESKHAND_AGENT_SOURCE", "local")
        .env("DESKHAND_LOGIN_RATE", "100")
        .env("RUST_LOG", "warn")
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, base: format!("http://127.0.0.1:{port}"), dir }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into()
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn read(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Resp {
    let mut r = r.unwrap();
    let headers = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned())).collect();
    let status = r.status().as_u16();
    let text = r.body_mut().read_to_string().unwrap_or_default();
    Resp { status, headers, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
}

fn get(url: &str, cookie: Option<&str>) -> Resp {
    let mut req = agent().get(url);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    read(req.call())
}

fn post_json(url: &str, cookie: Option<&str>, body: Value) -> Resp {
    let mut req = agent().post(url);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    read(req.send_json(body))
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Resp {
    read(agent().post(url).send_form(form.iter().copied()))
}

/// One JSON-RPC call to `/mcp` with a bearer token.
fn rpc(s: &Server, token: &str, method: &str, params: Value) -> Resp {
    read(
        agent()
            .post(format!("{}/mcp", s.base))
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-11-25")
            .send_json(json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })),
    )
}

fn tool(s: &Server, token: &str, name: &str, args: Value) -> Value {
    let r = rpc(s, token, "tools/call", json!({ "name": name, "arguments": args }));
    assert_eq!(r.status, 200, "{name}: {:?}", r.body);
    r.body["result"].clone()
}

fn login(s: &Server, user: &str, pw: &str) -> String {
    let r = post_json(&format!("{}/api/v1/login", s.base), None, json!({ "username": user, "password": pw }));
    assert_eq!(r.status, 200);
    r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned()
}

fn query_of(url: &str) -> Value {
    let q = url.split_once('?').map_or("", |(_, q)| q);
    Value::Object(url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), Value::String(v.into_owned()))).collect())
}

#[test]
fn mcp_oauth_and_tools() {
    let s = start();
    let b = &s.base;

    // Without a token: 401, pointing at the resource metadata.
    let r = rpc(&s, "nope", "tools/list", json!({}));
    assert_eq!(r.status, 401);
    let challenge = r.header("www-authenticate").unwrap();
    assert!(challenge.contains(&format!(r#"resource_metadata="{b}/.well-known/oauth-protected-resource/mcp""#)), "{challenge}");
    assert!(challenge.contains(r#"error="invalid_token""#));

    // Discovery.
    let prm = get(&format!("{b}/.well-known/oauth-protected-resource/mcp"), None).body;
    assert_eq!(prm["resource"], format!("{b}/mcp"));
    assert_eq!(prm["authorization_servers"][0], b.as_str());
    let asm = get(&format!("{b}/.well-known/oauth-authorization-server"), None).body;
    assert_eq!(asm["code_challenge_methods_supported"], json!(["S256"]));
    let (authorize, token, register) = (
        asm["authorization_endpoint"].as_str().unwrap().to_owned(),
        asm["token_endpoint"].as_str().unwrap().to_owned(),
        asm["registration_endpoint"].as_str().unwrap().to_owned(),
    );

    // Registration: https or loopback only.
    let redirect = "http://localhost:9999/callback";
    assert_eq!(post_json(&register, None, json!({ "redirect_uris": ["http://evil.example/cb"] })).status, 400);
    let reg = post_json(&register, None, json!({ "client_name": "Test", "redirect_uris": [redirect] }));
    assert_eq!(reg.status, 201);
    let client_id = reg.body["client_id"].as_str().unwrap().to_owned();

    // Authorize: checked, then handed to the web app's consent page.
    let verifier = "a-long-random-verifier-for-the-test-0123456789abcdef";
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.extend_pairs([
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", redirect),
        ("state", "st"),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("scope", "deskhand:read deskhand:act"),
        ("resource", &format!("{b}/mcp")),
    ]);
    let q = q.finish();
    let r = get(&format!("{authorize}?{q}"), None);
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location").unwrap(), format!("/connect?{q}"));
    // Without PKCE: back to the client with an error.
    let r = get(&format!("{authorize}?response_type=code&client_id={client_id}&redirect_uri={}&state=s", url::form_urlencoded::byte_serialize(redirect.as_bytes()).collect::<String>()), None);
    assert!(r.header("location").unwrap().starts_with(&format!("{redirect}?error=invalid_request")));

    // Consent: needs a session, refuses cross-site, then hands back a code.
    let mut decision = query_of(&format!("?{q}"));
    decision["allow"] = json!(true);
    decision["scopes"] = json!(["deskhand:read", "deskhand:act"]);
    assert_eq!(post_json(&format!("{b}/api/v1/mcp/authorize"), None, decision.clone()).status, 401);
    let admin = login(&s, "admin", "admin-password");
    let info = get(&format!("{b}/api/v1/mcp/authorize?{q}"), Some(&admin)).body;
    assert_eq!(info["local"], true);
    assert_eq!(info["client_name"], "Test");
    let cross = read(
        agent()
            .post(format!("{b}/api/v1/mcp/authorize"))
            .header("cookie", &admin)
            .header("origin", "https://evil.example")
            .send_json(decision.clone()),
    );
    assert_eq!(cross.status, 403);
    let code_for = |decision: &Value| {
        let r = post_json(&format!("{b}/api/v1/mcp/authorize"), Some(&admin), decision.clone());
        let back = query_of(r.body["redirect"].as_str().unwrap());
        assert_eq!(back["state"], "st");
        back["code"].as_str().unwrap().to_owned()
    };

    // Token: a wrong verifier spends the code; the right one gets a pair once.
    let code = code_for(&decision);
    let exchange = |code: &str, verifier: &str| {
        post_form(
            &token,
            &[("grant_type", "authorization_code"), ("client_id", &client_id), ("code", code), ("redirect_uri", redirect), ("code_verifier", verifier)],
        )
    };
    assert_eq!(exchange(&code, "wrong").body["error"], "invalid_grant");
    assert_eq!(exchange(&code, verifier).body["error"], "invalid_grant");
    let code = code_for(&decision);
    let pair = exchange(&code, verifier);
    assert_eq!(pair.status, 200, "{:?}", pair.body);
    assert_eq!(pair.body["scope"], "deskhand:read deskhand:act");
    assert_eq!(exchange(&code, verifier).status, 400);

    // Refresh rotates: the old refresh and access tokens stop working.
    let old_access = pair.body["access_token"].as_str().unwrap().to_owned();
    let old_refresh = pair.body["refresh_token"].as_str().unwrap().to_owned();
    let refresh = |rt: &str| post_form(&token, &[("grant_type", "refresh_token"), ("client_id", &client_id), ("refresh_token", rt)]);
    let next = refresh(&old_refresh);
    assert_eq!(next.status, 200);
    assert_eq!(refresh(&old_refresh).status, 400);
    assert_eq!(rpc(&s, &old_access, "tools/list", json!({})).status, 401);
    let access = next.body["access_token"].as_str().unwrap().to_owned();

    // MCP itself.
    let init = rpc(&s, &access, "initialize", json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }));
    assert_eq!(init.body["result"]["serverInfo"]["name"], "deskhand");
    let tools = rpc(&s, &access, "tools/list", json!({})).body;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["list_devices", "get_device", "run_command", "power_action", "get_job"] {
        assert!(names.contains(&want), "{names:?}");
    }
    let run = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "run_command").unwrap();
    assert_eq!(run["annotations"]["destructiveHint"], true);
    assert!(!run["inputSchema"].to_string().contains("\"null\""), "optional args aren't nullable");
    assert_eq!(rpc(&s, &access, "nope/nope", json!({})).body["error"]["code"], -32601);

    let me = tool(&s, &access, "whoami", json!({}));
    assert_eq!(me["structuredContent"]["role"], "admin");
    let list = tool(&s, &access, "list_devices", json!({ "limit": 5 }));
    assert_eq!(list["structuredContent"]["total"], 0);
    let missing = tool(&s, &access, "get_device", json!({ "device_id": "nope" }));
    assert_eq!(missing["isError"], true);
    // An inline command reaches the job API, which wants a target.
    let none = tool(&s, &access, "run_command", json!({ "shell": "bash", "command": "id" }));
    assert!(none["content"][0]["text"].as_str().unwrap().contains("selects no devices"), "{none}");

    // A read-only API token is a viewer; a viewer's full token is still a viewer.
    let ro = post_json(&format!("{b}/api/v1/mcp/tokens"), Some(&admin), json!({ "name": "ro", "scopes": ["deskhand:read"], "days": 1 }));
    let ro_token = ro.body["token"].as_str().unwrap().to_owned();
    assert_eq!(tool(&s, &ro_token, "whoami", json!({}))["structuredContent"]["role"], "viewer");
    let refused = tool(&s, &ro_token, "run_command", json!({ "shell": "bash", "command": "id", "all_devices": true }));
    assert!(refused["content"][0]["text"].as_str().unwrap().contains("read-only access"), "{refused}");
    assert_eq!(post_json(&format!("{b}/api/v1/users"), Some(&admin), json!({ "username": "vic", "password": "viewer-password", "role": "viewer" })).status, 200);
    let vic = login(&s, "vic", "viewer-password");
    let vt = post_json(&format!("{b}/api/v1/mcp/tokens"), Some(&vic), json!({ "name": "v", "scopes": ["deskhand:read", "deskhand:act"] }));
    let vt = vt.body["token"].as_str().unwrap().to_owned();
    let refused = tool(&s, &vt, "run_command", json!({ "shell": "bash", "command": "id", "all_devices": true }));
    assert!(refused["content"][0]["text"].as_str().unwrap().contains("vic is a viewer"), "{refused}");

    // Connections: your own; revoking ends the token at once; nobody else's to revoke.
    let mine = get(&format!("{b}/api/v1/mcp/connections"), Some(&vic)).body;
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    let ro_id = ro.body["id"].as_str().unwrap();
    let del = |cookie: &str| read(agent().delete(format!("{b}/api/v1/mcp/connections/{ro_id}")).header("cookie", cookie).call()).status;
    assert_eq!(del(&vic), 404);
    assert_eq!(del(&admin), 200);
    assert_eq!(rpc(&s, &ro_token, "tools/list", json!({})).status, 401);

    // The audit log names the person.
    let audit = get(&format!("{b}/api/v1/audit?limit=50"), Some(&admin)).body;
    let actions: Vec<&str> = audit["items"].as_array().unwrap().iter().map(|a| a["action"].as_str().unwrap()).collect();
    for want in ["mcp.connect", "mcp.token", "mcp.revoke"] {
        assert!(actions.contains(&want), "{actions:?}");
    }
}
