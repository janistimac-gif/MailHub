use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::db::{now_ms, Db};
use crate::gmail::{self, Api};
use crate::logging;
use crate::models::*;
use crate::oauth::{self, OAuthClient};

const FULL_SYNC_LIMIT: usize = 2000;
const PARALLEL: usize = 8;
const AUTH_PORT: u16 = 8765;

fn mask(value: &str) -> String {
    let head: String = value.chars().take(12).collect();
    format!("{head}…")
}

pub struct PendingLogin {
    pub redirect: String,
    pub state: String,
    pub auth_url: String,
}

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
    pub oauth: Mutex<Option<OAuthClient>>,
    pub tokens: Mutex<HashMap<String, (String, i64)>>,
    pub syncing: Mutex<HashSet<String>>,
    pub pending: Mutex<Option<PendingLogin>>,
    pub manual: Mutex<Option<tokio::sync::mpsc::UnboundedSender<(String, String)>>>,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        let oauth = match (db.get_setting("client_id"), db.get_setting("client_secret")) {
            (Ok(Some(id)), Ok(secret)) if !id.trim().is_empty() => Some(OAuthClient {
                client_id: id,
                client_secret: secret.filter(|s| !s.trim().is_empty()),
            }),
            _ => None,
        };
        Self {
            db,
            http: reqwest::Client::builder()
                .user_agent("MailHub/0.1")
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            oauth: Mutex::new(oauth),
            tokens: Mutex::new(HashMap::new()),
            syncing: Mutex::new(HashSet::new()),
            pending: Mutex::new(None),
            manual: Mutex::new(None),
        }
    }

    fn client(&self) -> Option<OAuthClient> {
        self.oauth.lock().unwrap().clone()
    }

    fn cache_token(&self, account_id: &str, token: &str, expires_in: i64) {
        let exp = now_ms() + expires_in.clamp(60, 86_400) * 1000;
        self.tokens
            .lock()
            .unwrap()
            .insert(account_id.to_string(), (token.to_string(), exp));
    }

    fn drop_token(&self, account_id: &str) {
        self.tokens.lock().unwrap().remove(account_id);
    }
}

// ------------------------------------------------------------------ tokens

async fn token_for(state: &AppState, account_id: &str) -> Result<String> {
    {
        let cache = state.tokens.lock().unwrap();
        if let Some((tok, exp)) = cache.get(account_id) {
            if *exp > now_ms() + 60_000 {
                return Ok(tok.clone());
            }
        }
    }
    let account = state
        .db
        .account(account_id)?
        .ok_or_else(|| anyhow!("Racun ne obstaja"))?;
    let client = state
        .client()
        .ok_or_else(|| anyhow!("OAuth Client ID ni nastavljen"))?;
    let stored = oauth::load_refresh_token(&account.email).or(state.db.refresh_token(account_id)?);
    let refresh = stored.ok_or_else(|| {
        anyhow!(
            "Za {} ni shranjenega zetona. Odstranite racun in ga dodajte znova.",
            account.email
        )
    })?;
    let tokens = oauth::refresh_token(&state.http, &client, &refresh)
        .await
        .map_err(|e| {
            anyhow!(
                "Prijava za {} je potekla ali je bila preklicana. Odstranite racun in ga \
                 dodajte znova (ali v Google Console objavite aplikacijo). Podrobnosti: {e}",
                account.email
            )
        })?;
    if let Some(new_token) = &tokens.refresh_token {
        if oauth::save_refresh_token(&account.email, new_token).is_err() {
            state.db.set_refresh_token(account_id, Some(new_token))?;
        }
    }
    state.cache_token(account_id, &tokens.access_token, tokens.expires_in);
    Ok(tokens.access_token)
}

// --------------------------------------------------------------- settings

#[tauri::command]
pub async fn setup_status(state: State<'_, AppState>) -> Result<SetupStatus, String> {
    let client = state.inner().client();
    Ok(SetupStatus {
        has_oauth_client: client.is_some(),
        has_secret: client
            .as_ref()
            .and_then(|c| c.client_secret.as_deref())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        client_id: client.map(|c| c.client_id),
    })
}

#[tauri::command]
pub async fn set_oauth_client(
    state: State<'_, AppState>,
    client_id: String,
    client_secret: Option<String>,
) -> Result<Option<String>, String> {
    let id = client_id.trim().to_string();
    if id.is_empty() {
        return Err("Client ID ne sme biti prazen.".into());
    }
    if !id.ends_with(".apps.googleusercontent.com") {
        return Err("Client ID se obicajno konca z .apps.googleusercontent.com".into());
    }
    let secret = client_secret.unwrap_or_default().trim().to_string();
    if secret.is_empty() {
        return Err(
            "Client secret je obvezen. Google ga zahteva pri zamenjavi prijavne kode za zeton \
             (tudi pri tipu »Desktop app«). Najdes ga v Google Cloud Console → Credentials → \
             tvoj OAuth client ID → Client secret (zacne se z GOCSPX-)."
                .into(),
        );
    }

    let client = OAuthClient {
        client_id: id.clone(),
        client_secret: Some(secret.clone()),
    };

    // Preveri pri Googlu, preden shranimo – tako napaka ne caka na koncu prijave.
    let mut warning: Option<String> = None;
    match oauth::probe_client(&state.http, &client).await {
        Ok(()) => {
            logging::log_line(format!("oauth client OK: {}", mask(&id)));
        }
        Err(e) => {
            let msg = e.to_string();
            logging::log_line(format!("oauth client FAILED: {} -> {msg}", mask(&id)));
            let network = msg.contains("network") || msg.contains("error sending request") || msg.contains("dns");
            if !network {
                return Err(msg);
            }
            warning = Some(
                "Nastavitve so shranjene, a preverjanja pri Googlu ni bilo mogoce izvesti \
                 (ni povezave). Prijava bo poskusena ob dodajanju racuna."
                    .to_string(),
            );
        }
    }

    state.db.set_setting("client_id", &id).map_err(|e| e.to_string())?;
    state
        .db
        .set_setting("client_secret", &secret)
        .map_err(|e| e.to_string())?;
    *state.inner().oauth.lock().unwrap() = Some(client);
    Ok(warning)
}

// --------------------------------------------------------------- accounts

#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    state.db.accounts().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    state: State<'_, AppState>,
    login_hint: Option<String>,
) -> Result<Account, String> {
    add_account_inner(&app, state.inner(), login_hint)
        .await
        .map_err(|e| e.to_string())
}

async fn add_account_inner(
    app: &AppHandle,
    state: &AppState,
    login_hint: Option<String>,
) -> Result<Account> {
    let client = state
        .client()
        .ok_or_else(|| anyhow!("Najprej vnesite OAuth Client ID in Client secret v nastavitvah."))?;

    if client
        .client_secret
        .as_deref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        bail!(
            "V Nastavitvah manjka Client secret. Google ga zahteva pri prijavi, zato brez njega \
             racuna ni mogoce povezati. Prilepi ga v polje »Client secret« in klikni Shrani."
        );
    }

    // Samo ena prijava hkrati – sicer se odpreta dva brskalnika in ena prijava poteče.
    if state.pending.lock().unwrap().is_some() {
        bail!(
            "Prijava je ze v teku. Dokoncajte jo v brskalniku (ali prilepite povezavo pod \
             »Rocna dokoncitev prijave«), preden dodate naslednji racun."
        );
    }

    logging::log_line(format!(
        "add_account: zacetek (hint: {})",
        login_hint.as_deref().unwrap_or("-")
    ));

    // Najprej poskusimo fiksna vrata (prijavljena v README za tip »Web application«);
    // pri tipu »Desktop app« Google dovoli poljubna vrata, zato ob zasedenosti
    // (npr. drugo orodje na 8765) mirno uporabimo prosto vrata.
    let listener = match TcpListener::bind(("127.0.0.1", AUTH_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            logging::log_line(format!(
                "add_account: port {AUTH_PORT} zaseden ({e}), uporabim prosto vrata"
            ));
            TcpListener::bind("127.0.0.1:0").await?
        }
    };
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}/");
    logging::log_line(format!("add_account: redirect {redirect}"));
    let (verifier, challenge) = oauth::pkce_pair();
    let state_param = oauth::random_token(32);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();

    let url = oauth::auth_url(
        &client.client_id,
        &redirect,
        &challenge,
        &state_param,
        login_hint.as_deref(),
    );
    logging::log_line(format!("add_account: auth URL {url}"));

    // Preveri prijavno povezavo se pred odprtjem brskalnika – ce Google ze takoj
    // vrne napako (npr. izbrisan odjemalec), uporabniku ni treba nic klikati.
    if let Err(e) = oauth::check_auth_url(&state.http, &url).await {
        logging::log_line(format!("add_account: prijavna povezava zavrnjena: {e}"));
        bail!("{e}");
    }

    *state.pending.lock().unwrap() = Some(PendingLogin {
        redirect: redirect.clone(),
        state: state_param.clone(),
        auth_url: url.clone(),
    });
    *state.manual.lock().unwrap() = Some(tx);

    use tauri_plugin_opener::OpenerExt;
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        clear_pending(state);
        bail!("Brskalnika ni bilo mogoce odpreti: {e}");
    }
    logging::log_line("add_account: brskalnik odprt, cakam na kodo");

    let outcome = wait_for_code(&listener, &mut rx).await;
    clear_pending(state);
    let (code, returned_state) = outcome?;
    logging::log_line("add_account: koda prejeta");
    if returned_state != state_param {
        bail!("Varnostno preverjanje (state) se ne ujema. Poskusite znova.");
    }

    let tokens = oauth::exchange_code(&state.http, &client, &code, &verifier, &redirect)
        .await
        .inspect_err(|e| logging::log_line(format!("add_account: izmenjava zetona ni uspela: {e}")))?;
    let info = oauth::userinfo(&state.http, &tokens.access_token).await?;
    let email = info
        .get("email")
        .and_then(|e| e.as_str())
        .ok_or_else(|| anyhow!("Google ni vrnil e-naslova"))?
        .to_string();
    logging::log_line(format!("add_account: prijavljen {email}"));
    let name = info
        .get("name")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();
    let picture = info.get("picture").and_then(|e| e.as_str()).map(str::to_string);

    let existing = state.db.account_by_email(&email)?;
    let id = existing
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    state.db.upsert_account(
        &id,
        &email,
        &name,
        picture.as_deref(),
        color_for(&email),
        None,
    )?;
    logging::log_line(format!("add_account: racun shranjen ({email}, id {id})"));

    if let Some(rt) = &tokens.refresh_token {
        if oauth::save_refresh_token(&email, rt).is_err() {
            state.db.set_refresh_token(&id, Some(rt))?;
        }
    }
    state.cache_token(&id, &tokens.access_token, tokens.expires_in);

    let account = state
        .db
        .account(&id)?
        .ok_or_else(|| anyhow!("Racuna ni bilo mogoce shraniti"))?;

    let handle = app.clone();
    let account_id = id.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let _ = sync_account_inner(&handle, state.inner(), &account_id).await;
    });

    Ok(account)
}

#[tauri::command]
pub async fn remove_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<(), String> {
    if let Some(acc) = state.db.account(&account_id).map_err(|e| e.to_string())? {
        oauth::delete_refresh_token(&acc.email);
    }
    state
        .db
        .remove_account(&account_id)
        .map_err(|e| e.to_string())?;
    state.inner().drop_token(&account_id);
    let _ = app.emit("mailhub:changed", json!({ "accountId": account_id }));
    Ok(())
}

// ------------------------------------------------------------ oauth server

fn clear_pending(state: &AppState) {
    *state.pending.lock().unwrap() = None;
    *state.manual.lock().unwrap() = None;
}

/// Iz poljubne oblike preusmeritve izvlece `code` in `state`.
/// Sprejme celoten URL, samo pot ali samo poizvedbo (`?code=...`).
fn parse_callback(target: &str) -> Result<Option<(String, String)>> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        bail!("Prilepite povezavo iz naslovne vrstice brskalnika.");
    }
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.starts_with('/') {
        format!("http://127.0.0.1{trimmed}")
    } else if trimmed.starts_with('?') {
        format!("http://127.0.0.1/{trimmed}")
    } else {
        format!("http://127.0.0.1/{trimmed}")
    };

    let parsed = url::Url::parse(&candidate).map_err(|_| {
        anyhow!("To ni veljavna povezava. Prekopirajte celoten naslov iz brskalnika.")
    })?;
    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .cloned()
            .unwrap_or_default();
        logging::log_line(format!("oauth: Google je vrnil napako {err} ({desc})"));
        if err == "access_denied" {
            bail!(
                "Google je dostop zavrnil (access_denied). Najpogosteje se to zgodi, ko tvoj \
                 e-naslov ni med »Test users« v Google Cloud Console, ali ko aplikacija ni \
                 objavljena. Odprite OAuth consent screen in dodajte svoj e-naslov med testne \
                 uporabnike (ali kliknite »Publish app«). Podrobnost: {desc}"
            );
        }
        if err == "invalid_client" {
            bail!(
                "Google ne prepozna OAuth odjemalca (invalid_client). Preverite Client ID in \
                 Client secret v Nastavitvah. Podrobnost: {desc}"
            );
        }
        bail!("Google je prijavo zavrnil ({err}). Podrobnost: {desc}");
    }

    match (params.get("code"), params.get("state")) {
        (Some(code), Some(state)) => Ok(Some((code.clone(), state.clone()))),
        (Some(_), None) => bail!("Povezavi manjka varnostni parameter (state)."),
        _ => Ok(None),
    }
}

/// Prebere eno zahtevo s prijavnega streznika. `None` pomeni, da zahteva ni
/// vsebovala kode (npr. favicon) in da cakamo naprej.
async fn handle_request(sock: &mut tokio::net::TcpStream) -> Option<Result<(String, String)>> {
    let mut buf = vec![0u8; 8192];
    let n = match tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => {
            let _ = sock.shutdown().await;
            return None;
        }
    };
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    logging::log_line(format!("oauth: zahteva: {path}"));

    if path.starts_with("/favicon") {
        respond(sock, "<p>.</p>").await;
        return None;
    }

    match parse_callback(&path) {
        Ok(Some(code)) => {
            respond(
                sock,
                "<h2>MailHub je povezan</h2><p>To okno lahko zaprete in se vrnite v aplikacijo.</p>",
            )
            .await;
            logging::log_line("oauth: koda prejeta prek lokalnega streznika");
            Some(Ok(code))
        }
        Ok(None) => {
            respond(sock, "<h2>Ni prijavne kode</h2><p>Vrnite se v MailHub in poskusite znova.</p>")
                .await;
            None
        }
        Err(e) => {
            logging::log_line(format!("oauth: neveljavna preusmeritev: {e}"));
            respond(
                sock,
                "<h2>Prijava ni dokoncana</h2><p>Vrnite se v MailHub in poskusite znova.</p>",
            )
            .await;
            Some(Err(e))
        }
    }
}

async fn wait_for_code(
    listener: &TcpListener,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, String)>,
) -> Result<(String, String)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            bail!("Prijava je potekla (5 minut). Poskusite znova.");
        }

        tokio::select! {
            accepted = listener.accept() => {
                let (mut sock, addr) = accepted?;
                logging::log_line(format!("oauth: povezava iz {addr}"));
                match handle_request(&mut sock).await {
                    Some(Ok(code)) => return Ok(code),
                    Some(Err(e)) => return Err(e),
                    None => continue,
                }
            }
            manual = rx.recv() => {
                match manual {
                    Some(code) => {
                        logging::log_line("oauth: koda prejeta z rocnim vnosom povezave");
                        return Ok(code);
                    }
                    None => bail!("Prijava je bila prekinjena. Poskusite znova."),
                }
            }
            _ = tokio::time::sleep(left) => {
                bail!("Prijava je potekla (5 minut). Poskusite znova.");
            }
        }
    }
}

/// Vrne prijavno povezavo, ki je trenutno v teku (za rocno odpiranje v brskalniku).
#[tauri::command]
pub async fn login_url(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state
        .pending
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.auth_url.clone()))
}

/// Preklice cakajoco prijavo (gumb »Prekliči« v nastavitvah).
#[tauri::command]
pub async fn cancel_login(state: State<'_, AppState>) -> Result<(), String> {
    let was_pending = state.pending.lock().unwrap().is_some();
    clear_pending(&state);
    logging::log_line(format!("add_account: prijava preklicana (v teku: {was_pending})"));
    Ok(())
}

/// Rocna dokoncitev prijave: uporabnik prilepi celoten naslov, na katerega ga je
/// preusmeril Google (uporabno, ko brskalnik ne more do lokalnega streznika).
#[tauri::command]
pub async fn submit_auth_url(state: State<'_, AppState>, url: String) -> Result<(), String> {
    let pending = {
        let guard = state.pending.lock().unwrap();
        guard
            .as_ref()
            .map(|p| (p.redirect.clone(), p.state.clone()))
            .ok_or_else(|| "Prijava ni v teku. Kliknite »Dodaj racun«.".to_string())?
    };

    let (code, returned_state) = parse_callback(&url)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "V povezavi ni prijavne kode. Prekopirajte celoten naslov iz brskalnika (zacne se z http://127.0.0.1:)."
                .to_string()
        })?;

    if returned_state != pending.1 {
        logging::log_line("manual: state se ne ujema");
        return Err(
            "Prilepljena povezava je od starejse prijave. Kliknite »Dodaj racun« in prilepite povezavo iz zadnjega brskalnika."
                .to_string(),
        );
    }

    logging::log_line(format!(
        "manual: prejeta povezava (pricakovana preusmeritev {})",
        pending.0
    ));

    let sender = state
        .manual
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Prijava ni v teku. Kliknite »Dodaj racun«.".to_string())?;
    sender
        .send((code, returned_state))
        .map_err(|_| "Prijava je bila medtem prekinjena. Kliknite »Dodaj racun«.".to_string())?;
    Ok(())
}

async fn respond(sock: &mut tokio::net::TcpStream, inner: &str) {
    let body = format!(
        "<!doctype html><html lang=\"sl\"><meta charset=\"utf-8\"><title>MailHub</title>\
         <body style=\"font-family:Segoe UI,system-ui,sans-serif;background:#0f1116;color:#e7e9ee;\
         display:flex;align-items:center;justify-content:center;height:100vh;margin:0\">\
         <div style=\"max-width:420px;text-align:center;padding:32px;border:1px solid #262b36;\
         border-radius:16px;background:#161923\">{inner}\
         <p style=\"color:#8b93a7;font-size:13px\">MailHub</p></div></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(),
        body
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.flush().await;
}

fn color_for(email: &str) -> &'static str {
    const PALETTE: [&str; 8] = [
        "#5b8def", "#e0679a", "#3fbf9a", "#e0a13f", "#9b6ee8", "#4fb8d8", "#d9694f", "#7fbf3f",
    ];
    let sum: usize = email.bytes().map(|b| b as usize).sum();
    PALETTE[sum % PALETTE.len()]
}

// ------------------------------------------------------------------- sync

#[tauri::command]
pub async fn sync_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<SyncReport, String> {
    sync_account_inner(&app, state.inner(), &account_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sync_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let accounts = state.db.accounts().map_err(|e| e.to_string())?;
    for acc in accounts {
        let handle = app.clone();
        let id = acc.id.clone();
        tauri::async_runtime::spawn(async move {
            let state = handle.state::<AppState>();
            let _ = sync_account_inner(&handle, state.inner(), &id).await;
        });
    }
    Ok(())
}

pub async fn sync_account_inner(
    app: &AppHandle,
    state: &AppState,
    account_id: &str,
) -> Result<SyncReport> {
    {
        let mut syncing = state.syncing.lock().unwrap();
        if !syncing.insert(account_id.to_string()) {
            return Ok(SyncReport {
                account_id: account_id.to_string(),
                ..Default::default()
            });
        }
    }
    let result = sync_once(state, account_id).await;
    state.syncing.lock().unwrap().remove(account_id);
    match result {
        Ok(report) => {
            logging::log_line(format!(
                "sync {account_id}: ok, prenesenih {}, odstranjenih {}",
                report.fetched,
                report.removed
            ));
            let _ = state.db.set_account_sync(account_id, None, None);
            let _ = app.emit("mailhub:changed", json!({ "accountId": account_id }));
            Ok(report)
        }
        Err(e) => {
            let msg = e.to_string();
            logging::log_line(format!("sync {account_id}: napaka: {msg}"));
            if msg.contains("401") {
                state.drop_token(account_id);
            }
            let _ = state.db.set_account_sync(account_id, None, Some(&msg));
            let _ = app.emit("mailhub:changed", json!({ "accountId": account_id }));
            Err(e)
        }
    }
}

async fn sync_once(state: &AppState, account_id: &str) -> Result<SyncReport> {
    let token = token_for(state, account_id).await?;
    let api = Api::new(&state.http, &token);

    // Profilna slika: poskusimo jo pridobiti, dokler je nimamo shranjene.
    if let Some(account) = state.db.account(account_id)? {
        if account.picture.is_none() {
            if let Ok(info) = oauth::userinfo(&state.http, &token).await {
                let picture = info
                    .get("picture")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = info.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let _ = state.db.set_account_profile(account_id, name, Some(&picture));
            }
        }
    }

    let report = if state.db.full_sync_done(account_id)? {
        incremental_sync(&api, &state.db, account_id).await?
    } else {
        full_sync(&api, &state.db, account_id).await?
    };

    if let Ok(labels) = sync_labels(&api, &state.db, account_id).await {
        let _ = labels;
    }
    Ok(report)
}

async fn full_sync(api: &Api<'_>, db: &Db, account_id: &str) -> Result<SyncReport> {
    let profile = api.profile().await?;
    let mut ids: Vec<String> = Vec::new();
    let mut page: Option<String> = None;
    let mut guard = 0;
    while ids.len() < FULL_SYNC_LIMIT && guard < 20 {
        guard += 1;
        let v = api.list_messages(None, page.as_deref(), 250, true).await?;
        if let Some(arr) = v.get("messages").and_then(|m| m.as_array()) {
            for m in arr {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
        page = v.get("nextPageToken").and_then(|p| p.as_str()).map(str::to_string);
        if page.is_none() {
            break;
        }
    }
    ids.truncate(FULL_SYNC_LIMIT);

    let msgs = fetch_metadata(api, account_id, &ids).await?;
    db.upsert_messages(account_id, &msgs)?;
    db.set_full_sync_done(account_id, true)?;
    if let Some(h) = profile.get("historyId").and_then(|h| h.as_str()) {
        db.set_account_sync(account_id, Some(h), None)?;
    }
    Ok(SyncReport {
        account_id: account_id.to_string(),
        fetched: msgs.len(),
        removed: 0,
        error: None,
    })
}

async fn incremental_sync(api: &Api<'_>, db: &Db, account_id: &str) -> Result<SyncReport> {
    let start = match db.history_id(account_id)? {
        Some(h) => h,
        None => {
            db.set_full_sync_done(account_id, false)?;
            return full_sync(api, db, account_id).await;
        }
    };

    let mut changed: HashSet<String> = HashSet::new();
    let mut deleted: HashSet<String> = HashSet::new();
    let mut page: Option<String> = None;
    let mut latest: Option<String> = None;

    loop {
        let v = match api.history(&start, page.as_deref()).await {
            Ok(v) => v,
            Err(e) if e.to_string().contains("NOT_FOUND") => {
                db.set_full_sync_done(account_id, false)?;
                return full_sync(api, db, account_id).await;
            }
            Err(e) => return Err(e),
        };
        if let Some(h) = v.get("historyId").and_then(|h| h.as_str()) {
            latest = Some(h.to_string());
        }
        if let Some(entries) = v.get("history").and_then(|h| h.as_array()) {
            for entry in entries {
                for key in ["messages", "messagesAdded", "labelsAdded", "labelsRemoved"] {
                    if let Some(items) = entry.get(key).and_then(|i| i.as_array()) {
                        for item in items {
                            let msg = item.get("message").unwrap_or(item);
                            if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
                                changed.insert(id.to_string());
                            }
                        }
                    }
                }
                if let Some(items) = entry.get("messagesDeleted").and_then(|i| i.as_array()) {
                    for item in items {
                        let msg = item.get("message").unwrap_or(item);
                        if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
                            deleted.insert(id.to_string());
                        }
                    }
                }
            }
        }
        page = v.get("nextPageToken").and_then(|p| p.as_str()).map(str::to_string);
        if page.is_none() {
            break;
        }
    }

    for id in &deleted {
        if !changed.contains(id) {
            db.delete_message(account_id, id)?;
        }
    }

    let ids: Vec<String> = changed.into_iter().collect();
    let msgs = fetch_metadata(api, account_id, &ids).await?;
    db.upsert_messages(account_id, &msgs)?;
    if let Some(h) = latest {
        db.set_account_sync(account_id, Some(&h), None)?;
    }

    Ok(SyncReport {
        account_id: account_id.to_string(),
        fetched: msgs.len(),
        removed: deleted.len(),
        error: None,
    })
}

async fn fetch_metadata(api: &Api<'_>, account_id: &str, ids: &[String]) -> Result<Vec<MessageSummary>> {
    let http = api.http.clone();
    let token = api.token.clone();
    let account = account_id.to_string();
    let mut out = Vec::with_capacity(ids.len());
    let mut first_error: Option<anyhow::Error> = None;
    let mut set: JoinSet<(String, Result<Value>)> = JoinSet::new();
    let mut iter = ids.iter();

    for _ in 0..PARALLEL {
        if let Some(id) = iter.next() {
            spawn_meta(&mut set, &http, &token, id);
        }
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_id, Ok(v))) => out.push(gmail::parse_metadata(&account, &v)),
            Ok((_id, Err(e))) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(anyhow!("{e}"));
                }
            }
        }
        if let Some(id) = iter.next() {
            spawn_meta(&mut set, &http, &token, id);
        }
    }
    if out.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
    }
    Ok(out)
}

fn spawn_meta(set: &mut JoinSet<(String, Result<Value>)>, http: &reqwest::Client, token: &str, id: &String) {
    let http = http.clone();
    let token = token.to_string();
    let id = id.clone();
    set.spawn(async move {
        let api = Api::new(&http, &token);
        let res = api.message_metadata(&id).await;
        (id, res)
    });
}

async fn sync_labels(api: &Api<'_>, db: &Db, account_id: &str) -> Result<usize> {
    let v = api.labels().await?;
    let mut labels = Vec::new();
    if let Some(arr) = v.get("labels").and_then(|l| l.as_array()) {
        for l in arr {
            labels.push(Label {
                account_id: account_id.to_string(),
                id: l.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                name: l.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                kind: l.get("type").and_then(|t| t.as_str()).unwrap_or("user").to_lowercase(),
                color: l
                    .get("color")
                    .and_then(|c| c.get("backgroundColor"))
                    .and_then(|c| c.as_str())
                    .map(str::to_string),
                unread: l.get("messagesUnread").and_then(|u| u.as_i64()).unwrap_or(0),
                total: l.get("messagesTotal").and_then(|t| t.as_i64()).unwrap_or(0),
            });
        }
    }
    db.replace_labels(account_id, &labels)?;
    Ok(labels.len())
}

// --------------------------------------------------------------- messages

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn list_messages(
    state: State<'_, AppState>,
    account_id: Option<String>,
    label: Option<String>,
    query: Option<String>,
    unread_only: Option<bool>,
    starred_only: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<MessageSummary>, String> {
    state
        .db
        .list_messages(
            account_id.as_deref(),
            label.as_deref(),
            query.as_deref(),
            unread_only.unwrap_or(false),
            starred_only.unwrap_or(false),
            limit.unwrap_or(200),
            offset.unwrap_or(0),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_labels(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<Vec<Label>, String> {
    state
        .db
        .labels(account_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_message(
    state: State<'_, AppState>,
    account_id: String,
    id: String,
) -> Result<MessageFull, String> {
    get_message_inner(state.inner(), &account_id, &id)
        .await
        .map_err(|e| e.to_string())
}

async fn get_message_inner(
    state: &AppState,
    account_id: &str,
    id: &str,
) -> Result<MessageFull> {
    let (mut summary, cached_text, cached_html, cached_headers) = state
        .db
        .message(account_id, id)?
        .ok_or_else(|| anyhow!("Sporocila ni v lokalni bazi. Osvezite nabiralnik."))?;

    if cached_text.is_some() || cached_html.is_some() {
        let headers = parse_headers(cached_headers.as_deref());
        let attachments = state.db.attachments(account_id, id)?;
        return Ok(build_full(summary, cached_text, cached_html, attachments, &headers));
    }

    let token = token_for(state, account_id).await?;
    let api = Api::new(&state.http, &token);
    let v = api.message_full(id).await?;
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let parsed = gmail::parse_body(&payload);
    let headers = gmail::header_map(&payload);
    let headers_json = serde_json::to_string(&headers).unwrap_or_default();

    state.db.save_body(
        account_id,
        id,
        parsed.text.as_deref(),
        parsed.html.as_deref(),
        Some(&headers_json),
    )?;
    state.db.replace_attachments(account_id, id, &parsed.attachments)?;

    summary = gmail::parse_metadata(account_id, &v);
    state.db.upsert_messages(account_id, &[summary.clone()])?;

    Ok(build_full(summary, parsed.text, parsed.html, parsed.attachments, &headers))
}

fn parse_headers(raw: Option<&str>) -> HashMap<String, String> {
    raw.and_then(|r| serde_json::from_str(r).ok()).unwrap_or_default()
}

fn build_full(
    summary: MessageSummary,
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<AttachmentMeta>,
    headers: &HashMap<String, String>,
) -> MessageFull {
    MessageFull {
        summary,
        body_text: text,
        body_html: html,
        attachments,
        references: headers.get("references").cloned(),
        rfc_message_id: headers.get("message-id").cloned(),
        reply_to: headers.get("reply-to").cloned(),
    }
}

#[tauri::command]
pub async fn search_messages(
    state: State<'_, AppState>,
    query: String,
    account_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<MessageSummary>, String> {
    search_inner(state.inner(), &query, account_id.as_deref(), limit.unwrap_or(40))
        .await
        .map_err(|e| e.to_string())
}

async fn search_inner(
    state: &AppState,
    query: &str,
    account_id: Option<&str>,
    per_account: u32,
) -> Result<Vec<MessageSummary>> {
    let accounts: Vec<Account> = match account_id {
        Some(id) => state.db.account(id)?.into_iter().collect(),
        None => state.db.accounts()?,
    };
    let mut all = Vec::new();
    for acc in accounts {
        let token = match token_for(state, &acc.id).await {
            Ok(t) => t,
            Err(_) => continue,
        };
        let api = Api::new(&state.http, &token);
        let v = match api.list_messages(Some(query), None, per_account, true).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ids: Vec<String> = v
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let msgs = fetch_metadata(&api, &acc.id, &ids).await.unwrap_or_default();
        state.db.upsert_messages(&acc.id, &msgs)?;
        all.extend(msgs);
    }
    all.sort_by(|a, b| b.date.cmp(&a.date));
    all.truncate((per_account as usize) * 4);
    Ok(all)
}

#[tauri::command]
pub async fn set_labels(
    state: State<'_, AppState>,
    account_id: String,
    id: String,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<Vec<String>, String> {
    set_labels_inner(state.inner(), &account_id, &id, &add, &remove)
        .await
        .map_err(|e| e.to_string())
}

async fn set_labels_inner(
    state: &AppState,
    account_id: &str,
    id: &str,
    add: &[String],
    remove: &[String],
) -> Result<Vec<String>> {
    let token = token_for(state, account_id).await?;
    let api = Api::new(&state.http, &token);
    let v = api.modify_labels(id, add, remove).await?;
    let labels = gmail::label_ids(&v);
    if !labels.is_empty() {
        state.db.update_labels(account_id, id, &labels)?;
    }
    Ok(labels)
}

#[tauri::command]
pub async fn trash_message(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    id: String,
    delete: bool,
) -> Result<(), String> {
    let token = token_for(state.inner(), &account_id)
        .await
        .map_err(|e| e.to_string())?;
    let api = Api::new(&state.http, &token);
    let v = if delete {
        api.trash(&id).await
    } else {
        api.untrash(&id).await
    }
    .map_err(|e| e.to_string())?;
    let labels = gmail::label_ids(&v);
    if !labels.is_empty() {
        state
            .db
            .update_labels(&account_id, &id, &labels)
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("mailhub:changed", json!({ "accountId": account_id }));
    Ok(())
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SendInput,
) -> Result<String, String> {
    send_inner(&app, state.inner(), input)
        .await
        .map_err(|e| e.to_string())
}

async fn send_inner(app: &AppHandle, state: &AppState, input: SendInput) -> Result<String> {
    let account = state
        .db
        .account(&input.account_id)?
        .ok_or_else(|| anyhow!("Racun ne obstaja"))?;
    if input.to.trim().is_empty() {
        bail!("Prejemnik je obvezen.");
    }
    let raw = gmail::build_raw(&gmail::Outgoing {
        from_name: &account.name,
        from_email: &account.email,
        to: &input.to,
        cc: input.cc.as_deref().unwrap_or(""),
        bcc: input.bcc.as_deref().unwrap_or(""),
        subject: &input.subject,
        body_text: &input.body_text,
        body_html: input.body_html.as_deref(),
        in_reply_to: input.in_reply_to.as_deref(),
        references: input.references.as_deref(),
    });

    let token = token_for(state, &input.account_id).await?;
    let api = Api::new(&state.http, &token);
    let v = api.send(&raw, input.thread_id.as_deref()).await?;
    let sent_id = v
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();

    let handle = app.clone();
    let account_id = input.account_id.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let _ = sync_account_inner(&handle, state.inner(), &account_id).await;
    });
    Ok(sent_id)
}

#[tauri::command]
pub async fn download_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
    attachment_id: String,
    filename: String,
) -> Result<String, String> {
    let token = token_for(state.inner(), &account_id)
        .await
        .map_err(|e| e.to_string())?;
    let api = Api::new(&state.http, &token);
    let bytes = api
        .attachment(&message_id, &attachment_id)
        .await
        .map_err(|e| e.to_string())?;

    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().app_data_dir())
        .map_err(|e| e.to_string())?;
    let safe = sanitize_filename(&filename);
    let mut path = dir.join(&safe);
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (safe.clone(), String::new()),
    };
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}{ext}"));
        n += 1;
    }
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\r' | '\n' => '_',
            c => c,
        })
        .collect();
    if cleaned.trim().is_empty() {
        "priponka".to_string()
    } else {
        cleaned
    }
}

#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if path.starts_with("http://") || path.starts_with("https://") {
        return app
            .opener()
            .open_url(path, None::<&str>)
            .map_err(|e| e.to_string());
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_log(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = logging::log_dir()
        .map(|dir| dir.join("mailhub.log"))
        .ok_or_else(|| "Mape APPDATA ni bilo mogoce najti.".to_string())?;
    if !path.exists() {
        return Err(
            "Dnevnika se ni. Ustvari se ob prijavi ali prvi napaki sinhronizacije.".to_string(),
        );
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn autostart_enable(app: AppHandle) -> Result<(), String> {
    let path = app.path().app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mailhub.exe");
    let mut auto = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("MailHub")
        .set_app_path(&path.to_string_lossy())
        .set_use_launch_agent(true)
        .build()
        .map_err(|e| e.to_string())?;
    auto.enable().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn autostart_disable(app: AppHandle) -> Result<(), String> {
    let path = app.path().app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mailhub.exe");
    let mut auto = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("MailHub")
        .set_app_path(&path.to_string_lossy())
        .set_use_launch_agent(true)
        .build()
        .map_err(|e| e.to_string())?;
    auto.disable().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn autostart_is_enabled(app: AppHandle) -> Result<bool, String> {
    let path = app.path().app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mailhub.exe");
    let auto = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("MailHub")
        .set_app_path(&path.to_string_lossy())
        .set_use_launch_agent(true)
        .build()
        .map_err(|e| e.to_string())?;
    auto.is_enabled().map_err(|e| e.to_string())
}
