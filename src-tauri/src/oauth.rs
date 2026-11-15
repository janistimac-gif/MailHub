use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gmail::{AUTH_URL, SCOPES, TOKEN_URL, USERINFO_URL};

pub const KEYRING_SERVICE: &str = "com.jbj.mailhub";

#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
}

pub fn random_token(len: usize) -> String {
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(&buf).chars().take(len).collect()
}

pub fn pkce_pair() -> (String, String) {
    let verifier = random_token(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn auth_url(
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
    login_hint: Option<&str>,
) -> String {
    let mut url = url::Url::parse(AUTH_URL).expect("auth url");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent")
            .append_pair("include_granted_scopes", "true");
        if let Some(hint) = login_hint {
            if !hint.trim().is_empty() {
                q.append_pair("login_hint", hint.trim());
            }
        }
    }
    url.to_string()
}

async fn token_request(
    http: &reqwest::Client,
    form: &[(&str, String)],
) -> Result<Tokens> {
    let resp = http.post(TOKEN_URL).form(form).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let err = v
            .get("error_description")
            .or_else(|| v.get("error"))
            .and_then(|e| e.as_str())
            .unwrap_or(&text);
        if err.contains("client_secret is missing") || err.contains("client_secret is invalid") {
            bail!(
                "Google zahteva Client secret. Odpri Nastavitve in prilepi Client secret \
                 (GOCSPX-…) istega OAuth odjemalca, nato poskusi znova."
            );
        }
        if err.contains("redirect_uri_mismatch") {
            bail!(
                "Google ne dovoli preusmeritve na http://127.0.0.1:8765/. OAuth odjemalec mora \
                 biti tipa »Desktop app«. Ustvari nov OAuth client ID s tem tipom."
            );
        }
        if err.contains("invalid_grant") {
            bail!(
                "Prijava je potekla ali je bila že uporabljena. Zapri brskalnik in klikni \
                 »Dodaj račun« znova."
            );
        }
        bail!("Google prijava ni uspela: {err}");
    }
    let access = v
        .get("access_token")
        .and_then(|a| a.as_str())
        .ok_or_else(|| anyhow!("odgovor brez access_token"))?;
    Ok(Tokens {
        access_token: access.to_string(),
        refresh_token: v
            .get("refresh_token")
            .and_then(|r| r.as_str())
            .map(str::to_string),
        expires_in: v.get("expires_in").and_then(|e| e.as_i64()).unwrap_or(3600),
    })
}

pub async fn exchange_code(
    http: &reqwest::Client,
    client: &OAuthClient,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<Tokens> {
    let mut form = vec![
        ("code", code.to_string()),
        ("client_id", client.client_id.clone()),
        ("grant_type", "authorization_code".to_string()),
        ("code_verifier", verifier.to_string()),
        ("redirect_uri", redirect.to_string()),
    ];
    if let Some(secret) = &client.client_secret {
        if !secret.trim().is_empty() {
            form.push(("client_secret", secret.trim().to_string()));
        }
    }
    token_request(http, &form).await
}

pub async fn refresh_token(
    http: &reqwest::Client,
    client: &OAuthClient,
    refresh: &str,
) -> Result<Tokens> {
    let mut form = vec![
        ("client_id", client.client_id.clone()),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh.to_string()),
    ];
    if let Some(secret) = &client.client_secret {
        if !secret.trim().is_empty() {
            form.push(("client_secret", secret.trim().to_string()));
        }
    }
    token_request(http, &form).await
}

pub async fn userinfo(http: &reqwest::Client, access_token: &str) -> Result<Value> {
    let resp = http.get(USERINFO_URL).bearer_auth(access_token).send().await?;
    if !resp.status().is_success() {
        bail!("branje profila ni uspelo");
    }
    Ok(resp.json::<Value>().await?)
}

/// Preveri, ali Google pozna ta Client ID + secret, še preden se uporabnik prijavi.
/// Uporabi namerno neveljavno kodo: veljaven odjemalec vrne `invalid_grant`,
/// napačen ali nepopoln pa drugačno napako.
pub async fn probe_client(http: &reqwest::Client, client: &OAuthClient) -> Result<()> {
    let mut form = vec![
        ("code", "mailhub-preverjanje".to_string()),
        ("client_id", client.client_id.clone()),
        ("grant_type", "authorization_code".to_string()),
        ("code_verifier", "A".repeat(64)),
        ("redirect_uri", "http://127.0.0.1:8765/".to_string()),
    ];
    if let Some(secret) = client.client_secret.as_deref() {
        if !secret.trim().is_empty() {
            form.push(("client_secret", secret.trim().to_string()));
        }
    }
    let resp = http.post(TOKEN_URL).form(&form).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
    let desc = v
        .get("error_description")
        .and_then(|e| e.as_str())
        .unwrap_or(text.as_str());

    if status.is_success() || err == "invalid_grant" || desc.contains("Malformed auth code") {
        return Ok(());
    }
    if desc.contains("client_secret is missing") || desc.contains("client_secret is invalid") {
        bail!(
            "Google zahteva Client secret za ta OAuth odjemalec. V Google Cloud Console → \
             Credentials → tvoj OAuth client → prekopiraj Client secret (začne se z GOCSPX-) \
             in ga prilepi v polje »Client secret«."
        );
    }
    if err == "deleted_client" || desc.to_lowercase().contains("was deleted") {
        bail!(
            "Ta OAuth odjemalec je bil v Google Cloud Console izbrisan. Ustvari novega: \
             Credentials → Create credentials → OAuth client ID → Desktop app, ter prilepi \
             nov Client ID in Client secret."
        );
    }
    if err == "invalid_client" || desc.to_lowercase().contains("unauthorized") {
        bail!(
            "Google ne prepozna tega Client ID in Client secret para. Preveri, da si prekopiral \
             oba podatka iz istega OAuth odjemalca (Credentials v Google Cloud Console)."
        );
    }
    bail!("Google je zavrnil odjemalca ({status}): {desc}");
}

/// Iz Googlove napake `authError` izluzi kodo in opis (base64 protobuf).
fn decode_auth_error(final_url: &str) -> (String, String) {
    let raw = url::Url::parse(final_url)
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "authError")
                .map(|(_, v)| v.to_string())
        })
        .unwrap_or_default();
    if raw.is_empty() {
        return (String::new(), String::new());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(raw.as_bytes()))
        .unwrap_or_default();

    let mut runs: Vec<String> = Vec::new();
    let mut cur = String::new();
    for byte in bytes {
        if (32..127).contains(&byte) {
            cur.push(byte as char);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }

    let code = runs
        .iter()
        .find(|r| {
            r.len() > 4 && r.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        })
        .cloned()
        .unwrap_or_default();
    let desc = runs
        .iter()
        .find(|r| **r != code && r.len() > 10)
        .cloned()
        .unwrap_or_default();
    (code, desc)
}

/// Preveri prijavno povezavo pri Googlu se pred odprtjem brskalnika.
/// Ce Google ze takoj vrne stran z napako (izbrisan odjemalec, neveljavna
/// preusmeritev ...), uporabniku prihranimo brezplodno prijavo.
pub async fn check_auth_url(http: &reqwest::Client, auth_url: &str) -> Result<()> {
    let resp = match http
        .get(auth_url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // brez povezave ne blokiramo prijave
            crate::logging::log_line(format!("oauth: preverjanje povezave ni mozno: {e}"));
            return Ok(());
        }
    };

    let final_url = resp.url().to_string();
    if !final_url.contains("/signin/oauth/error") && !final_url.contains("authError=") {
        return Ok(());
    }

    let (code, desc) = decode_auth_error(&final_url);
    crate::logging::log_line(format!("oauth: Google javi napako v prijavni povezavi: {code} ({desc})"));
    match code.as_str() {
        "deleted_client" => bail!(
            "Ta OAuth odjemalec je bil v Google Cloud Console izbrisan, zato Google prijave ne \
             dovoli. Ustvari novega: Credentials → Create credentials → OAuth client ID → \
             Desktop app, nato v Nastavitvah vpisi nov Client ID in Client secret."
        ),
        "redirect_uri_mismatch" => bail!(
            "Google ne dovoli preusmeritve na lokalni naslov. V Google Cloud Console odpri \
             Credentials → tvoj OAuth client in med Authorized redirect URIs dodaj \
             http://127.0.0.1:8765/ (oziroma ustvari odjemalec tipa »Desktop app«)."
        ),
        "invalid_client" | "unauthorized_client" => bail!(
            "Google ne prepozna OAuth odjemalca. Preveri, da sta Client ID in Client secret iz \
             istega odjemalca v Google Cloud Console."
        ),
        "access_denied" => bail!(
            "Google je dostop zavrnil (access_denied). V Google Cloud Console → OAuth consent \
             screen dodaj svoj e-naslov med »Test users« ali objavi aplikacijo (»Publish app«)."
        ),
        _ => bail!("Google je prijavno povezavo zavrnil ({code}): {desc}"),
    }
}

// --------------------------------------------------------- OS token store
fn entry(email: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, email).map_err(|e| anyhow!("keyring: {e}"))
}

pub fn save_refresh_token(email: &str, token: &str) -> Result<()> {
    entry(email)?
        .set_password(token)
        .map_err(|e| anyhow!("keyring set: {e}"))
}

pub fn load_refresh_token(email: &str) -> Option<String> {
    entry(email).ok()?.get_password().ok()
}

pub fn delete_refresh_token(email: &str) {
    if let Ok(e) = entry(email) {
        let _ = e.delete_credential();
    }
}
