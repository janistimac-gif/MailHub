# MailHub

A desktop app (Rust + Tauri 2) that unifies **multiple Gmail accounts into one inbox**:
one message list, one search, one reading pane, one compose window.

![status](https://img.shields.io/badge/status-prototype-blue)

## Features

- **Multiple accounts simultaneously** – Google OAuth login (no passwords stored), each account gets its own color and **profile picture** (fallback: initials in account color).
- **Unified inbox** – Inbox, Starred, Sent, Drafts, All Mail, Spam, Trash + custom labels.
- **Search across all accounts** – uses Gmail syntax (`from:`, `to:`, `subject:`, `has:attachment`, `after:2026/01/01`, …).
- **Reading** – HTML or plain text, links and images; images blocked by default (click to show).
- **Send & reply** – choose sender account, proper `In-Reply-To` / `References` threading.
- **Labels, stars, read/unread, archive, trash**.
- **Attachments** – download to `Downloads` folder.
- **Background sync** – every minute, incremental via Gmail `historyId` (doesn't re-fetch entire mailbox).
- **Local SQLite DB** – works offline; tokens stored in Windows Credential Manager (keyring on Linux/macOS).
- **Auto-start on login** (minimized to background) – toggle in Settings.
- **Delete from message list** – trash button directly on each row.

## Requirements

- Windows 10/11 (WebView2 is built-in), Linux, or macOS
- [Rust](https://rustup.rs) with **MSVC** toolchain on Windows (Visual Studio Build Tools)
- Node.js 20+

> ⚠️ On some Windows systems, `PATH` may have a GNU Rust toolchain first which cannot link Tauri apps.
> Use the provided `dev.ps1` / `dev.cmd` (they prepend `$USERPROFILE\.cargo\bin` to PATH).

## 1. Google OAuth Setup (one-time, ~5 minutes)

The app needs **your own** OAuth client. It's free and your data stays with you.

1. Open <https://console.cloud.google.com/> and create a project (e.g., `MailHub`).
2. **APIs & Services → Library** → search **Gmail API** → *Enable*.
3. **APIs & Services → OAuth consent screen**:
   - User type: **External**,
   - App name e.g. `MailHub`, your email as contact,
   - Under **Test users** add **all Gmail addresses** you'll connect.
4. **APIs & Services → Credentials → Create credentials → OAuth client ID**:
   - Application type: **Desktop app**,
   - Copy **Client ID** *and* **Client secret** (starts with `GOCSPX-`).
     Google requires the secret even for Desktop app type.
   - If you accidentally create **Web application**, add
     *Authorized redirect URI*: `http://127.0.0.1:8765/` (MailHub uses this port).
5. Launch MailHub, open **Settings** and paste Client ID/Secret → *Save & verify*.
   The app validates the client with Google immediately.
6. In **Settings**, below the account list, are quick-add buttons for your addresses –
   click each, sign in via browser, allow access.
   The list is in `src/main.ts` (`QUICK_ACCOUNTS`), edit as needed.

> Each account must be added **separately** – Google requires a separate login per account
> (password and 2FA stay with Google, the app never sees them).

> Only **one login at a time** is allowed. If the browser doesn't return to the app after sign-in,
> open **Manual login completion** in Settings and paste the full URL from the browser
> (starts with `http://127.0.0.1:8765/?code=…&state=…`).

### Important: Token Expiry in "Testing" Mode

While the app is in **Testing** state in Google Console, Google limits refresh tokens to **7 days**.
When it expires, remove the account in MailHub and add it again (or publish the app:
*OAuth consent screen → Publish app*). For personal use, unverified publishing works fine –
you'll just see an "unverified app" warning during sign-in.

## 2. Running the App

```powershell
cd <repo-root>
.\dev.ps1
```

Or double-click `dev.cmd` (no PowerShell execution policy changes needed).

Or manually:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
npm run tauri dev
```

### Building Installers / Standalone Binary

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
npm run tauri build
```

Output: `src-tauri\target\release\bundle\`

| Artifact | Location |
|---|---|
| **NSIS installer (.exe)** | `src-tauri\target\release\bundle\nsis\MailHub_0.1.0_x64-setup.exe` |
| **MSI installer (.msi)** | `src-tauri\target\release\bundle\msi\MailHub_0.1.0_x64.msi` |
| **Standalone binary** | `src-tauri\target\release\mailhub.exe` (no Node/Rust needed) |

**First run:** Windows SmartScreen → *More info* → *Run anyway*.

## Data Locations

| What | Where |
|---|---|
| Database (accounts, messages, labels) | `%APPDATA%\com.jbj.mailhub\mailhub.db` |
| Logs (auth, sync) | `%APPDATA%\com.jbj.mailhub\mailhub.log` – *Open log* button in Settings |
| Refresh tokens | Windows Credential Manager (`com.jbj.mailhub`) / keyring on Linux/macOS |
| Downloaded attachments | `%USERPROFILE%\Downloads` |

Passwords are **never stored** – only OAuth tokens. Revoke access anytime at
<https://myaccount.google.com/permissions>.

## Architecture

```
src/                     frontend (TypeScript, no framework)
  api.ts                 typed wrapper around Tauri commands
  main.ts                state + rendering (list, reader, modals)
  styles.css
src-tauri/src/
  lib.rs                 app entry, background refresh loop
  commands.rs            all Tauri commands + OAuth loopback server + sync
  gmail.rs               Gmail REST API, MIME building, RFC 2047 decoding
  oauth.rs               PKCE, token exchange, keyring
  db.rs                  SQLite schema & queries
  models.rs              data types (serde camelCase)
```

### Sync Flow

1. First run per account: `messages.list` (up to 2000 newest, incl. spam/trash) →
   parallel `messages.get?format=metadata` (8 concurrent) → SQLite.
2. Record `historyId` from `users.getProfile`.
3. Subsequent runs: `users.history.list` from last `historyId` → changes (added, labels, deleted).
4. On `404` (history expired) → automatic full re-sync.
5. Every 60 s, accounts not synced in the last minute are refreshed.

### Tauri Commands

`setup_status`, `set_oauth_client`, `list_accounts`, `add_account`, `remove_account`,
`sync_account`, `sync_all`, `list_messages`, `list_labels`, `get_message`, `search_messages`,
`set_labels`, `trash_message`, `send_message`, `download_attachment`, `open_path`, `open_log`,
`autostart_enable`, `autostart_disable`, `autostart_is_enabled`.

## Known Limitations (v0.1)

- **Gmail only** (Google APIs). IMAP/POP and Outlook not supported.
- First sync fetches newest **2000** messages per account (`FULL_SYNC_LIMIT` in `commands.rs`).
- Search runs **on Google's server** (like Gmail), so it needs internet;
  folder/account filtering works locally.
- **Attachments cannot be sent** yet (only received/downloaded).
- HTML email renders in `iframe sandbox` without scripts – safe, but links open inside frame.
- App is **not code-signed**; Windows SmartScreen prompts on first run.
- Google may require CASA verification for public distribution – not needed for personal use.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `error calling dlltool 'dlltool.exe'` | Use `dev.cmd` / `dev.ps1` – PATH must have rustup MSVC, not "Rust stable GNU". |
| Window is blank (white/white) | Set `"csp": null` in `src-tauri/tauri.conf.json` and restart. |
| "Login expired or revoked" | App is in Google Console *Testing* mode (token expires in 7 days). Remove account and re-add, or publish app (*Publish app*). |
| `client_secret is missing` / "Missing Client secret" | In Settings paste **Client secret** (GOCSPX-…) from the same OAuth client and click *Save & verify*. |
| `redirect_uri_mismatch` | OAuth client must be **Desktop app** type (not Web application). If Web, add `http://127.0.0.1:8765/` to *Authorized redirect URIs*. |
| Account won't sync | Check ⚠ icon next to account in Settings – shows last error. |
| Browser doesn't return to app after sign-in (blank page / nothing happens) | Open **Manual login completion** in Settings, copy full URL from browser (starts with `http://127.0.0.1:8765/?code=…`) paste and click *Complete login*. |
| "Login already in progress" | Only one login at a time. Finish the open one (browser or manual) before adding another. Cancel with *Cancel login wait*. |
| Port 8765 used by another tool (e.g., OBS StreamDock polls it every 3 s) | No problem: MailHub picks a free port `127.0.0.1:<port>` on conflict, which Google allows for *Desktop app* type. |

## Contributing

1. Fork the repo
2. Create a feature branch
3. Make your changes
4. Run `npm run tauri dev` to test
5. Submit a PR

## License

MIT License – see [LICENSE](LICENSE) for details.