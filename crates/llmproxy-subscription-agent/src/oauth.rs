//! 官方动态公共客户端 OAuth；不读取或使用 Codex token。
use crate::{Error, credentials::StateDirectory, now, random_id};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};
use url::Url;

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

#[derive(Deserialize, Serialize)]
pub struct Credentials {
    pub client_id: String,
    pub subject: String,
    pub host_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub scopes: String,
    pub expires_at: u64,
}

#[derive(Deserialize, Serialize)]
struct Token {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
    earliest_refresh_at: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    token: Token,
    received_at: u64,
    previous_access_digest: String,
}

fn fingerprint(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: serde_json::Value,
    sub: String,
    exp: u64,
    nbf: Option<u64>,
    nonce: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}
#[derive(Deserialize)]
struct Key {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    alg: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
}
#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Key>,
}

pub fn client() -> Result<reqwest::Client, Error> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

async fn json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
    if !response.status().is_success() {
        return Err("OpenAI 认证请求被拒绝，请重新登录或检查权限".into());
    }
    let bytes = crate::http::limited_response(response, 1024 * 1024).await?;
    serde_json::from_slice(&bytes).map_err(|_| "OpenAI 认证响应无效".into())
}

async fn validate_id(
    client: &reqwest::Client,
    token: &str,
    audience: &str,
    nonce: Option<&str>,
) -> Result<Claims, Error> {
    let jwks: Jwks = json(
        client
            .get(format!("{ISSUER}/.well-known/jwks.json"))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| "无法取得 OpenAI 签名公钥")?,
    )
    .await?;
    verify_id(token, audience, nonce, &jwks, now()?)
}

fn verify_id(
    token: &str,
    audience: &str,
    nonce: Option<&str>,
    jwks: &Jwks,
    time: u64,
) -> Result<Claims, Error> {
    if token.len() > 64 * 1024 {
        return Err("ID token 超过容量上限".into());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("ID token 格式无效".into());
    }
    let header: Header = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[0])
            .map_err(|_| "ID token 头无效")?,
    )
    .map_err(|_| "ID token 头无效")?;
    if header.alg != "RS256" {
        return Err("不支持的 OpenAI ID token 签名算法".into());
    }
    let key = jwks
        .keys
        .iter()
        .find(|key| {
            key.kid == header.kid
                && key.kty == "RSA"
                && key.alg.as_deref().is_none_or(|alg| alg == "RS256")
                && key.usage.as_deref().is_none_or(|usage| usage == "sig")
        })
        .ok_or("OpenAI 签名公钥未找到")?;
    let n = URL_SAFE_NO_PAD
        .decode(key.n.as_deref().ok_or("RSA 公钥无效")?)
        .map_err(|_| "RSA 公钥无效")?;
    let e = URL_SAFE_NO_PAD
        .decode(key.e.as_deref().ok_or("RSA 公钥无效")?)
        .map_err(|_| "RSA 公钥无效")?;
    let signature = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| "ID token 签名无效")?;
    ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &signature,
        )
        .map_err(|_| "ID token 签名校验失败")?;
    let claims: Claims = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| "ID token 载荷无效")?,
    )
    .map_err(|_| "ID token 载荷无效")?;
    let aud = claims.aud.as_str() == Some(audience)
        || claims
            .aud
            .as_array()
            .is_some_and(|values| values.len() == 1 && values[0].as_str() == Some(audience));
    if claims.iss != ISSUER
        || !aud
        || claims.sub.is_empty()
        || claims.exp <= time
        || claims.nbf.is_some_and(|value| value > time)
        || nonce.is_some_and(|value| claims.nonce.as_deref() != Some(value))
    {
        return Err("ID token 身份、有效期或 nonce 校验失败".into());
    }
    Ok(claims)
}

fn validate_token(token: &Token, scopes: &str) -> Result<(), Error> {
    if !token.token_type.eq_ignore_ascii_case("bearer")
        || token.access_token.is_empty()
        || token.refresh_token.is_empty()
        || !scopes
            .split_whitespace()
            .any(|scope| scope == "chatgpt.tokens.use.direct")
        || token.expires_in == 0
    {
        return Err("未取得有效 ChatGPT 订阅授权".into());
    }
    Ok(())
}

// UUID host identifiers must be URIs, not the random strings used for state/PKCE.
fn prepare_host_id(stored: Option<String>, registered: Option<&str>) -> Result<String, Error> {
    if let Some(host) = stored.or_else(|| registered.map(str::to_owned)) {
        let legacy = host.len() == 64 && host.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !legacy {
            return Ok(host);
        }
        if registered.is_some() {
            return Err("已有 OAuth 注册使用旧 host ID；不能自动修改已注册身份".into());
        }
        // The old hexadecimal format never passed authorization validation.
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "无法生成 OAuth host ID")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub async fn login(directory: &StateDirectory) -> Result<(), Error> {
    let _lock = directory.lock()?;
    let previous = directory.read::<Credentials>("oauth.json")?;
    let host = prepare_host_id(
        directory.read::<String>("host.json")?,
        previous.as_ref().map(|p| p.host_id.as_str()),
    )?;
    directory.write("host.json", &host)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let state = random_id()?;
    let nonce = random_id()?;
    let verifier = random_id()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&format!("{ISSUER}/api/accounts/authorize"))?;
    url.query_pairs_mut().extend_pairs([
        (
            "client_id",
            previous
                .as_ref()
                .map(|p| p.client_id.as_str())
                .unwrap_or("dynamic_agent_client"),
        ),
        ("ext_agent_host_id", &host),
        ("response_type", "code"),
        ("redirect_uri", &redirect),
        ("scope", SCOPES),
        ("resource", RESOURCE),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
    ]);
    if previous.is_none() {
        url.query_pairs_mut()
            .append_pair("agent_name_hint", "llmproxy-subscription-agent");
    }
    // 不把含 ID token 的返回登录 URL 打印到终端。
    if let Some(previous) = &previous {
        url.query_pairs_mut()
            .append_pair("id_token_hint", &previous.id_token);
    }
    let launch = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let opened = tokio::process::Command::new(launch)
        .arg(url.as_str())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    if !opened.is_ok_and(|status| status.success()) {
        if previous.is_some() {
            return Err("无法打开登录浏览器；请在桌面环境重新授权".into());
        }
        println!("Continue with ChatGPT：{url}");
    } else {
        println!("请在浏览器完成 Continue with ChatGPT 授权。");
    }
    let client = client()?;
    let callback=tokio::time::timeout(Duration::from_secs(900), async {
        loop {
            let (mut socket,_)=listener.accept().await?;
            let mut bytes=Vec::new(); let mut buf=[0;1024];
            let read=tokio::time::timeout(Duration::from_secs(5),async {
                while !bytes.windows(4).any(|p|p==b"\r\n\r\n") {
                    let n=socket.read(&mut buf).await?; if n==0 || bytes.len()+n>16*1024 { return Err("OAuth 回调过大或截断".into()); }
                    bytes.extend_from_slice(&buf[..n]);
                }
                Ok::<_,Error>(())
            }).await;
            if !matches!(read,Ok(Ok(()))) { continue; }
            let request=std::str::from_utf8(&bytes).map_err(|_| "OAuth 回调无效")?;
            let target=request.lines().next().and_then(|line|line.strip_prefix("GET ")).and_then(|line|line.strip_suffix(" HTTP/1.1"));
            let parsed=target.and_then(|target| Url::parse(&format!("http://127.0.0.1{target}")).ok());
            let pairs=parsed.as_ref().map(|u|u.query_pairs().into_owned().collect::<Vec<_>>()).unwrap_or_default();
            let fields: HashMap<_,_>=pairs.iter().cloned().collect();
            let valid=parsed.is_some_and(|u|u.path()=="/auth/callback") && fields.get("state")==Some(&state) && pairs.len()==fields.len();
            let response=if valid { "HTTP/1.1 200 OK\r\nContent-Length: 27\r\nConnection: close\r\n\r\nAuthorization received.\n\n\n\n" } else { "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" };
            let _=socket.write_all(response.as_bytes()).await;
            if valid { return Ok::<_,Error>(fields); }
        }
    }).await.map_err(|_| "OAuth 登录等待超时")??;
    if callback.contains_key("error") {
        return Err("用户拒绝授权或 OAuth 登录失败".into());
    }
    let issued = match (&previous, callback.get("client_id")) {
        (Some(previous), None) => previous.client_id.clone(),
        (Some(previous), Some(value)) if value == &previous.client_id => value.clone(),
        (None, Some(value)) if value != "dynamic_agent_client" && !value.is_empty() => {
            value.clone()
        }
        _ => return Err("OAuth 回调 client_id 缺失或与当前账号不一致".into()),
    };
    let code = callback
        .get("code")
        .filter(|value| !value.is_empty())
        .ok_or("OAuth 回调缺少 code")?;
    let token: Token = tokio::time::timeout(Duration::from_secs(30), async {
        json(
            client
                .post(format!("{ISSUER}/api/accounts/oauth/token"))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", &issued),
                    ("code", code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect),
                    ("resource", RESOURCE),
                ])
                .send()
                .await
                .map_err(|_| "OAuth token 交换失败")?,
        )
        .await
    })
    .await
    .map_err(|_| "OAuth token 交换超时")??;
    let scopes = token
        .scope
        .as_deref()
        .ok_or("OAuth token 响应缺少授权范围")?;
    validate_token(&token, scopes)?;
    let id = token
        .id_token
        .as_deref()
        .ok_or("OAuth token 响应缺少 ID token")?;
    let claims = validate_id(&client, id, &issued, Some(&nonce)).await?;
    if previous.as_ref().is_some_and(|p| p.subject != claims.sub) {
        return Err("登录账号与已有注册不一致；请使用独立状态目录".into());
    }
    directory.write(
        "oauth.json",
        &Credentials {
            client_id: issued,
            subject: claims.sub,
            host_id: host,
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token.unwrap(),
            scopes: scopes.to_owned(),
            expires_at: now()?
                .checked_add(token.expires_in)
                .ok_or("token 有效期无效")?,
        },
    )?;
    println!("ChatGPT 授权完成；凭据已保存到本地受保护文件。");
    Ok(())
}

pub struct Session {
    directory: Arc<StateDirectory>,
    credentials: Mutex<Credentials>,
    client: reqwest::Client,
}

impl Session {
    pub fn load(directory: Arc<StateDirectory>) -> Result<Self, Error> {
        let credentials: Credentials = directory
            .read("oauth.json")?
            .ok_or("请先执行 login，不能用 Codex 凭据替代 OAuth")?;
        if credentials.client_id.is_empty()
            || credentials.client_id == "dynamic_agent_client"
            || credentials.subject.is_empty()
            || credentials.host_id.is_empty()
            || credentials.access_token.is_empty()
            || credentials.refresh_token.is_empty()
            || !credentials
                .scopes
                .split_whitespace()
                .any(|scope| scope == "chatgpt.tokens.use.direct")
        {
            return Err("本地凭据不包含有效的 ChatGPT 订阅授权，请重新登录".into());
        }
        Ok(Self {
            directory,
            credentials: Mutex::new(credentials),
            client: client()?,
        })
    }
    pub async fn token(&self) -> Result<String, Error> {
        let mut saved = self.credentials.lock().await;
        // 旋转成功后先保存 pending；崩溃或公钥网络错误后从 pending 继续校验，不能再用旧 refresh token。
        let mut pending = self.directory.read::<Pending>("oauth-pending.json")?;
        if pending.as_ref().is_some_and(|pending| {
            pending.previous_access_digest != fingerprint(&saved.access_token)
        }) {
            std::fs::remove_file(self.directory.0.join("oauth-pending.json"))?;
            pending = None;
        }
        if pending.is_none() && saved.expires_at > now()?.saturating_add(60) {
            return Ok(saved.access_token.clone());
        }
        let pending = match pending {
            Some(pending) => pending,
            None => {
                let token: Token = tokio::time::timeout(Duration::from_secs(30), async {
                    json(
                        self.client
                            .post(format!("{ISSUER}/api/accounts/oauth/token"))
                            .form(&[
                                ("grant_type", "refresh_token"),
                                ("client_id", saved.client_id.as_str()),
                                ("refresh_token", saved.refresh_token.as_str()),
                                ("resource", RESOURCE),
                            ])
                            .send()
                            .await
                            .map_err(|_| "OAuth 刷新请求失败，请重新授权")?,
                    )
                    .await
                })
                .await
                .map_err(|_| "OAuth 刷新超时")??;
                let pending = Pending {
                    token,
                    received_at: now()?,
                    previous_access_digest: fingerprint(&saved.access_token),
                };
                self.directory.write("oauth-pending.json", &pending)?;
                pending
            }
        };
        let token = pending.token;
        let scopes = token.scope.as_deref().unwrap_or(&saved.scopes);
        validate_token(&token, scopes)?;
        if let Some(id) = &token.id_token {
            let claims = validate_id(&self.client, id, &saved.client_id, None).await?;
            if claims.sub != saved.subject {
                return Err("刷新后的 OAuth 身份与已有账号不一致".into());
            }
        }
        let replacement = Credentials {
            client_id: saved.client_id.clone(),
            subject: saved.subject.clone(),
            host_id: saved.host_id.clone(),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token.unwrap_or_else(|| saved.id_token.clone()),
            scopes: scopes.to_owned(),
            expires_at: pending
                .received_at
                .checked_add(token.expires_in)
                .ok_or("token 有效期无效")?,
        };
        self.directory.write("oauth.json", &replacement)?;
        *saved = replacement;
        std::fs::remove_file(self.directory.0.join("oauth-pending.json"))?;
        Ok(saved.access_token.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_host_uri_is_stable_and_migrates_only_unregistered_legacy_ids() {
        let fresh = prepare_host_id(None, None).unwrap();
        assert!(fresh.starts_with("urn:uuid:"));
        let uuid = fresh.strip_prefix("urn:uuid:").unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(prepare_host_id(Some(fresh.clone()), None).unwrap(), fresh);
        assert_eq!(prepare_host_id(None, Some(&fresh)).unwrap(), fresh);
        let legacy = "a".repeat(64);
        assert!(
            prepare_host_id(Some(legacy.clone()), None)
                .unwrap()
                .starts_with("urn:uuid:")
        );
        assert!(prepare_host_id(Some(legacy.clone()), Some(&legacy)).is_err());
    }

    #[test]
    fn signed_identity_checks_signature_issuer_audience_expiry_and_nonce() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/public-jwt.json")).unwrap();
        let token = fixture["token"].as_str().unwrap();
        let jwks: Jwks = serde_json::from_value(fixture["jwks"].clone()).unwrap();
        assert_eq!(
            verify_id(
                token,
                "test-client",
                Some("test-nonce"),
                &jwks,
                1_800_000_000
            )
            .unwrap()
            .sub,
            "test-subject"
        );
        assert!(
            verify_id(
                token,
                "other-client",
                Some("test-nonce"),
                &jwks,
                1_800_000_000
            )
            .is_err()
        );
        assert!(verify_id(token, "test-client", Some("wrong"), &jwks, 1_800_000_000).is_err());
        assert!(verify_id(token, "test-client", None, &jwks, 2_000_000_000).is_err());
        assert!(verify_id(token, "test-client", None, &jwks, 1_600_000_000).is_err());
        let mut parts = token.split('.').map(str::to_owned).collect::<Vec<_>>();
        let mut claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&parts[1]).unwrap()).unwrap();
        claims["iss"] = serde_json::json!("https://attacker.example");
        parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        assert!(verify_id(&parts.join("."), "test-client", None, &jwks, 1_800_000_000).is_err());
        assert!(
            verify_id(
                token,
                "test-client",
                None,
                &Jwks { keys: vec![] },
                1_800_000_000
            )
            .is_err()
        );
    }
}
