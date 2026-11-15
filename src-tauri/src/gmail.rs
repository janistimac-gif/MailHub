use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use serde_json::{json, Value};

use crate::models::{AttachmentMeta, MessageSummary};

pub const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";
pub const SCOPES: &str = "openid email profile https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send";

pub struct Api<'a> {
    pub http: &'a reqwest::Client,
    pub token: String,
}

impl<'a> Api<'a> {
    pub fn new(http: &'a reqwest::Client, token: &str) -> Self {
        Self { http, token: token.to_string() }
    }

    async fn get(&self, url: &str) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("omrezna napaka")?;
        handle(resp).await
    }

    async fn post(&self, url: &str, body: Value) -> Result<Value> {
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("omrezna napaka")?;
        handle(resp).await
    }

    pub async fn profile(&self) -> Result<Value> {
        self.get(&format!("{API}/profile")).await
    }

    pub async fn labels(&self) -> Result<Value> {
        self.get(&format!("{API}/labels")).await
    }

    pub async fn list_messages(
        &self,
        q: Option<&str>,
        page_token: Option<&str>,
        max_results: u32,
        include_spam_trash: bool,
    ) -> Result<Value> {
        let mut url = format!("{API}/messages?maxResults={max_results}");
        if let Some(q) = q {
            if !q.is_empty() {
                url.push_str("&q=");
                url.push_str(&urlencoding(q));
            }
        }
        if include_spam_trash {
            url.push_str("&includeSpamTrash=true");
        }
        if let Some(p) = page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(p));
        }
        self.get(&url).await
    }

    pub async fn message_metadata(&self, id: &str) -> Result<Value> {
        let headers = [
            "Subject",
            "From",
            "To",
            "Cc",
            "Date",
            "Message-ID",
            "References",
            "Reply-To",
        ]
        .iter()
        .map(|h| format!("metadataHeaders={h}"))
        .collect::<Vec<_>>()
        .join("&");
        self.get(&format!("{API}/messages/{id}?format=metadata&{headers}")).await
    }

    pub async fn message_full(&self, id: &str) -> Result<Value> {
        self.get(&format!("{API}/messages/{id}?format=full")).await
    }

    pub async fn modify_labels(&self, id: &str, add: &[String], remove: &[String]) -> Result<Value> {
        self.post(
            &format!("{API}/messages/{id}/modify"),
            json!({ "addLabelIds": add, "removeLabelIds": remove }),
        )
        .await
    }

    pub async fn trash(&self, id: &str) -> Result<Value> {
        self.post(&format!("{API}/messages/{id}/trash"), json!({})).await
    }

    pub async fn untrash(&self, id: &str) -> Result<Value> {
        self.post(&format!("{API}/messages/{id}/untrash"), json!({})).await
    }

    pub async fn send(&self, raw: &str, thread_id: Option<&str>) -> Result<Value> {
        let mut body = json!({ "raw": raw });
        if let Some(t) = thread_id {
            body["threadId"] = json!(t);
        }
        self.post(&format!("{API}/messages/send"), body).await
    }

    pub async fn history(&self, start: &str, page_token: Option<&str>) -> Result<Value> {
        let mut url = format!("{API}/history?startHistoryId={start}&maxResults=500");
        if let Some(p) = page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(p));
        }
        self.get(&url).await
    }

    pub async fn attachment(&self, message_id: &str, attachment_id: &str) -> Result<Vec<u8>> {
        let v = self
            .get(&format!("{API}/messages/{message_id}/attachments/{attachment_id}"))
            .await?;
        let data = v.get("data").and_then(|d| d.as_str()).unwrap_or("");
        b64url_decode(data)
    }
}

async fn handle(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_str(&text).context("neveljaven JSON iz Gmail API");
    }
    if status.as_u16() == 401 {
        bail!("Google je zavrnil dostop (401). Prijavite racun znova.");
    }
    if status.as_u16() == 404 {
        bail!("NOT_FOUND");
    }
    let short: String = text.chars().take(400).collect();
    bail!("Gmail API napaka {status}: {short}")
}

// ---------------------------------------------------------------- parsing

pub fn parse_metadata(account_id: &str, v: &Value) -> MessageSummary {
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let headers = header_map(&payload);
    let labels = label_ids(v);
    let (from_name, from_email) = split_address(headers.get("from").map(String::as_str).unwrap_or(""));
    let has_attach = payload
        .get("parts")
        .and_then(|p| p.as_array())
        .map(|parts| parts.iter().any(has_attachment))
        .unwrap_or_else(|| !payload.get("filename").and_then(|f| f.as_str()).unwrap_or("").is_empty());

    MessageSummary {
        account_id: account_id.to_string(),
        id: v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        thread_id: v.get("threadId").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        unread: labels.iter().any(|l| l == "UNREAD"),
        starred: labels.iter().any(|l| l == "STARRED"),
        label_ids: labels,
        snippet: v.get("snippet").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        subject: headers.get("subject").cloned().unwrap_or_default(),
        from_name,
        from_email,
        to: headers.get("to").cloned().unwrap_or_default(),
        date: v
            .get("internalDate")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0),
        has_attachments: has_attach,
    }
}

fn has_attachment(part: &Value) -> bool {
    if !part.get("filename").and_then(|f| f.as_str()).unwrap_or("").is_empty() {
        return true;
    }
    part.get("parts")
        .and_then(|p| p.as_array())
        .map(|ps| ps.iter().any(has_attachment))
        .unwrap_or(false)
}

pub fn label_ids(v: &Value) -> Vec<String> {
    v.get("labelIds")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

pub fn header_map(payload: &Value) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(headers) = payload.get("headers").and_then(|h| h.as_array()) {
        for h in headers {
            let name = h.get("name").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
            let value = decode_header(h.get("value").and_then(|x| x.as_str()).unwrap_or(""));
            map.entry(name).or_insert(value);
        }
    }
    map
}

pub struct ParsedBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<AttachmentMeta>,
}

pub fn parse_body(payload: &Value) -> ParsedBody {
    let mut text = String::new();
    let mut html = String::new();
    let mut attachments = Vec::new();
    walk(payload, &mut text, &mut html, &mut attachments);
    ParsedBody {
        text: if text.trim().is_empty() { None } else { Some(text.trim().to_string()) },
        html: if html.trim().is_empty() { None } else { Some(html.trim().to_string()) },
        attachments,
    }
}

fn walk(part: &Value, text: &mut String, html: &mut String, atts: &mut Vec<AttachmentMeta>) {
    let mime = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    let filename = part.get("filename").and_then(|f| f.as_str()).unwrap_or("");
    let body = part.get("body").cloned().unwrap_or(Value::Null);
    let attachment_id = body.get("attachmentId").and_then(|a| a.as_str());

    if !filename.is_empty() {
        if let Some(aid) = attachment_id {
            atts.push(AttachmentMeta {
                part_id: aid.to_string(),
                filename: filename.to_string(),
                mime_type: mime.to_string(),
                size: body.get("size").and_then(|s| s.as_i64()).unwrap_or(0),
            });
        }
    }

    if let Some(parts) = part.get("parts").and_then(|p| p.as_array()) {
        for p in parts {
            walk(p, text, html, atts);
        }
        return;
    }

    if filename.is_empty() {
        if let Some(data) = body.get("data").and_then(|d| d.as_str()) {
            if let Ok(bytes) = b64url_decode(data) {
                let content = decode_charset(&bytes, charset_of(part));
                match mime {
                    "text/plain" => {
                        text.push_str(&content);
                        text.push_str("\n\n");
                    }
                    "text/html" => {
                        html.push_str(&content);
                        html.push_str("\n\n");
                    }
                    _ => {}
                }
            }
        }
    }
}

fn charset_of(part: &Value) -> &str {
    part.get("mimeType")
        .and_then(|m| m.as_str())
        .and_then(|m| m.split("charset=").nth(1))
        .unwrap_or("utf-8")
}

fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let label = charset.trim_matches('"').trim();
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (cow, _, _) = enc.decode(bytes);
    cow.into_owned()
}

pub fn split_address(input: &str) -> (String, String) {
    let input = input.trim();
    if let (Some(start), Some(end)) = (input.rfind('<'), input.rfind('>')) {
        if end > start {
            let email = input[start + 1..end].trim().to_string();
            let name = input[..start].trim().trim_matches('"').trim().to_string();
            return (decode_header(&name), email);
        }
    }
    (String::new(), input.to_string())
}

/// Decodes RFC 2047 encoded words (=?UTF-8?B?...?=, =?iso-8859-2?Q?...?=).
pub fn decode_header(input: &str) -> String {
    if !input.contains("=?") {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut i = 0usize;
    let mut last_was_encoded = false;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"=?") {
            if let Some(end) = find_sub(&bytes[i + 2..], b"?=") {
                let word = &input[i + 2..i + 2 + end];
                let mut parts = word.splitn(3, '?');
                let charset = parts.next().unwrap_or("utf-8");
                let enc = parts.next().unwrap_or("B");
                let encoded = parts.next().unwrap_or("");
                let decoded: Option<String> = if enc.eq_ignore_ascii_case("B") {
                    b64_decode(encoded).map(|bytes| decode_charset(&bytes, charset))
                } else {
                    Some(decode_q(encoded, charset))
                };
                if let Some(d) = decoded {
                    out.push_str(&d);
                    last_was_encoded = true;
                    i = i + 2 + end + 2;
                    continue;
                }
            }
        }
        // plain text run
        let ch_len = utf8_char_len(bytes[i]);
        let ch = &input[i..(i + ch_len).min(input.len())];
        if !(last_was_encoded && ch == " ") {
            out.push_str(ch);
        }
        last_was_encoded = false;
        i += ch_len;
    }
    out.trim().to_string()
}

fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

fn decode_q(s: &str, charset: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => {
                bytes.push(b' ');
                i += 1;
            }
            b'=' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    bytes.push(v);
                    i += 3;
                } else {
                    bytes.push(b[i]);
                    i += 1;
                }
            }
            c => {
                bytes.push(c);
                i += 1;
            }
        }
    }
    decode_charset(&bytes, charset)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn b64_decode(data: &str) -> Option<Vec<u8>> {
    let bytes = decode_with(&B64, data)
        .or_else(|| decode_with(&B64URL, data))?;
    Some(bytes)
}

fn decode_with<E: Engine>(engine: &E, data: &str) -> Option<Vec<u8>> {
    let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    if let Ok(bytes) = engine.decode(&cleaned) {
        return Some(bytes);
    }
    let trimmed = cleaned.trim_end_matches('=');
    let pad = (4 - trimmed.len() % 4) % 4;
    engine.decode(format!("{trimmed}{}", "=".repeat(pad))).ok()
}

pub fn b64url_decode(data: &str) -> Result<Vec<u8>> {
    decode_with(&B64URL, data)
        .or_else(|| decode_with(&B64, data))
        .ok_or_else(|| anyhow!("base64: neveljavni podatki"))
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------- building

pub struct Outgoing<'a> {
    pub from_name: &'a str,
    pub from_email: &'a str,
    pub to: &'a str,
    pub cc: &'a str,
    pub bcc: &'a str,
    pub subject: &'a str,
    pub body_text: &'a str,
    pub body_html: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub references: Option<&'a str>,
}

/// Builds an RFC 2822 message and returns it base64url-encoded (Gmail `raw`).
pub fn build_raw(msg: &Outgoing<'_>) -> String {
    let boundary = format!("mailhub-{}", uuid::Uuid::new_v4().simple());
    let domain = msg.from_email.split('@').nth(1).unwrap_or("mailhub.local");
    let mut h = String::new();
    h.push_str(&format!("From: {}\r\n", mailbox(msg.from_name, msg.from_email)));
    h.push_str(&format!("To: {}\r\n", msg.to));
    if !msg.cc.trim().is_empty() {
        h.push_str(&format!("Cc: {}\r\n", msg.cc));
    }
    if !msg.bcc.trim().is_empty() {
        h.push_str(&format!("Bcc: {}\r\n", msg.bcc));
    }
    h.push_str(&format!("Subject: {}\r\n", encode_header(msg.subject)));
    h.push_str(&format!(
        "Date: {}\r\n",
        chrono::Local::now().to_rfc2822()
    ));
    h.push_str(&format!(
        "Message-ID: <{}@{}>\r\n",
        uuid::Uuid::new_v4().simple(),
        domain
    ));
    if let Some(irt) = msg.in_reply_to {
        h.push_str(&format!("In-Reply-To: {irt}\r\n"));
    }
    if let Some(r) = msg.references {
        h.push_str(&format!("References: {r}\r\n"));
    }
    h.push_str("MIME-Version: 1.0\r\n");

    let mut body = String::new();
    match msg.body_html {
        Some(html) if !html.trim().is_empty() => {
            h.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n"
            ));
            body.push_str(&format!("--{boundary}\r\n"));
            body.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
            body.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            body.push_str(&wrap64(&B64.encode(msg.body_text.as_bytes())));
            body.push_str(&format!("\r\n--{boundary}\r\n"));
            body.push_str("Content-Type: text/html; charset=UTF-8\r\n");
            body.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            body.push_str(&wrap64(&B64.encode(html.as_bytes())));
            body.push_str(&format!("\r\n--{boundary}--\r\n"));
        }
        _ => {
            h.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
            h.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            body.push_str(&wrap64(&B64.encode(msg.body_text.as_bytes())));
            body.push_str("\r\n");
        }
    }

    let raw = format!("{h}{body}");
    B64URL.encode(raw.as_bytes())
}

fn mailbox(name: &str, email: &str) -> String {
    if name.trim().is_empty() {
        email.to_string()
    } else {
        format!("{} <{}>", encode_header(name), email)
    }
}

pub fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.replace(['\r', '\n'], " ");
    }
    let encoded = B64.encode(value.as_bytes());
    format!("=?UTF-8?B?{encoded}?=")
}

fn wrap64(s: &str) -> String {
    s.as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_word() {
        assert_eq!(decode_header("=?UTF-8?B?SGVsbG8=?="), "Hello");
    }

    #[test]
    fn decodes_q_word() {
        assert_eq!(decode_header("=?UTF-8?Q?Po=C5=A1ta?="), "Pošta");
    }

    #[test]
    fn decodes_mixed_subject() {
        assert_eq!(
            decode_header("=?UTF-8?B?UG96ZHJhdmxqZW5p?= =?UTF-8?B?IGRhbg==?="),
            "Pozdravljeni dan"
        );
    }

    #[test]
    fn splits_display_address() {
        let (name, email) = split_address("Janez Novak <janez@example.com>");
        assert_eq!(name, "Janez Novak");
        assert_eq!(email, "janez@example.com");
        let (_, bare) = split_address("ana@example.com");
        assert_eq!(bare, "ana@example.com");
    }

    #[test]
    fn builds_mime_message() {
        let raw = build_raw(&Outgoing {
            from_name: "Ziga",
            from_email: "ziga@example.com",
            to: "ana@example.com",
            cc: "",
            bcc: "",
            subject: "Pozdravljeni – čšž",
            body_text: "Zdravo!",
            body_html: Some("<p>Zdravo!</p>"),
            in_reply_to: None,
            references: None,
        });
        let bytes = b64url_decode(&raw).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("From: Ziga <ziga@example.com>"));
        assert!(text.contains("To: ana@example.com"));
        assert!(text.contains("Subject: =?UTF-8?B?"));
        assert!(text.contains("multipart/alternative"));
        assert!(text.contains("Content-Transfer-Encoding: base64"));
    }

    #[test]
    fn parses_gmail_payload() {
        let payload = serde_json::json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/plain", "body": { "data": "VGVrc3Q=" } },
                { "mimeType": "text/html", "body": { "data": "PGI+aGk8L2I+" } }
            ]
        });
        let parsed = parse_body(&payload);
        assert_eq!(parsed.text.as_deref(), Some("Tekst"));
        assert_eq!(parsed.html.as_deref(), Some("<b>hi</b>"));
    }
}
