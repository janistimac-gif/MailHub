import "./styles.css";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type Account,
  type Label,
  type MessageFull,
  type MessageSummary,
  type ListOptions,
  type SendInput,
} from "./api";

const PAGE = 100;

interface State {
  accounts: Account[];
  labels: Label[];
  messages: MessageSummary[];
  selected: MessageFull | null;
  selectedId: string | null;
  accountFilter: string | null;
  folder: string | null;
  query: string;
  loading: boolean;
  offset: number;
  hasClient: boolean;
  hasSecret: boolean;
  showImages: boolean;
}

const state: State = {
  accounts: [],
  labels: [],
  messages: [],
  selected: null,
  selectedId: null,
  accountFilter: null,
  folder: "INBOX",
  query: "",
  loading: false,
  offset: 0,
  hasClient: false,
  hasSecret: false,
  showImages: false,
};

interface Folder {
  id: string | null;
  label: string;
  icon: string;
}

const FOLDERS: Folder[] = [
  { id: "INBOX", label: "Prejeto", icon: "📥" },
  { id: "STARRED", label: "Z zvezdico", icon: "⭐" },
  { id: "SENT", label: "Poslano", icon: "📤" },
  { id: "DRAFT", label: "Osnutki", icon: "📝" },
  { id: null, label: "Vsa pošta", icon: "🗂" },
  { id: "SPAM", label: "Neželena", icon: "🚫" },
  { id: "TRASH", label: "Smeti", icon: "🗑" },
];

// ------------------------------------------------------------- utilities

function el<T extends HTMLElement>(selector: string): T {
  return document.querySelector(selector) as T;
}

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function fmtDate(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString("sl-SI", { hour: "2-digit", minute: "2-digit" });
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString("sl-SI", { day: "numeric", month: "short" });
  }
  return d.toLocaleDateString("sl-SI");
}

function fmtFull(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleString("sl-SI", { dateStyle: "medium", timeStyle: "short" });
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} kB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function toast(message: string, kind: "info" | "error" | "ok" = "info"): void {
  const root = el<HTMLDivElement>("#toast-root");
  const node = document.createElement("div");
  node.className = `toast ${kind === "info" ? "" : kind}`;
  node.textContent = message;
  root.appendChild(node);
  setTimeout(() => {
    node.style.opacity = "0";
    node.style.transition = "opacity .25s";
    setTimeout(() => node.remove(), 260);
  }, kind === "error" ? 6500 : 3200);
}

function accountById(id: string): Account | undefined {
  return state.accounts.find((a) => a.id === id);
}

function accountColor(id: string): string {
  return accountById(id)?.color ?? "#5b8def";
}

function accountEmail(id: string): string {
  return accountById(id)?.email ?? "neznan račun";
}

/** Ikona računa: Google profilna slika, sicer začetnica v barvi računa. */
function avatarHtml(account: Account | undefined, fallbackEmail: string, size = 20): string {
  const color = account?.color ?? "#5b8def";
  const email = account?.email ?? fallbackEmail;
  const initial = (account?.name?.trim() || email || "?").slice(0, 1).toUpperCase();
  const style =
    `width:${size}px;height:${size}px;font-size:${Math.max(9, Math.round(size * 0.46))}px;` +
    `background:${color}`;
  if (account?.picture) {
    return `<span class="avatar" style="${style}" title="${esc(email)}"><img data-avatar src="${esc(
      account.picture,
    )}" alt="" /><span class="initial" hidden>${esc(initial)}</span></span>`;
  }
  return `<span class="avatar" style="${style}" title="${esc(email)}"><span class="initial">${esc(
    initial,
  )}</span></span>`;
}

/** Če se slika ne naloži (npr. brez povezave), pokaži začetnico. */
function wireAvatars(root: ParentNode): void {
  root.querySelectorAll<HTMLImageElement>("img[data-avatar]").forEach((img) => {
    img.addEventListener("error", () => {
      img.remove();
      const initial = img.parentElement?.querySelector<HTMLElement>(".initial");
      if (initial) initial.hidden = false;
    });
  });
}

function folderLabel(id: string | null): string {
  return FOLDERS.find((f) => f.id === id)?.label ?? id ?? "Vsa pošta";
}

// ------------------------------------------------------------ rendering

function renderAccounts(): void {
  const nav = el<HTMLDivElement>("#accounts-nav");
  const totalUnread = state.accounts.reduce((sum, a) => sum + a.unread, 0);

  let html = `
    <button class="side-item account ${state.accountFilter === null ? "active" : ""}" data-account="">
      <span class="avatar all" style="width:20px;height:20px;font-size:10px">✉</span>
      <span class="name">Vsi računi</span>
      ${totalUnread > 0 ? `<span class="badge">${totalUnread}</span>` : ""}
    </button>`;

  for (const acc of state.accounts) {
    html += `
      <button class="side-item account ${state.accountFilter === acc.id ? "active" : ""}"
              data-account="${esc(acc.id)}" title="${esc(acc.email)}">
        ${avatarHtml(acc, acc.email, 20)}
        <span class="name">${esc(acc.name || acc.email.split("@")[0])}</span>
        ${acc.syncError ? `<span class="badge" title="${esc(acc.syncError)}">⚠</span>` : ""}
        ${acc.unread > 0 ? `<span class="badge">${acc.unread}</span>` : ""}
      </button>`;
  }

  nav.innerHTML = html;
  wireAvatars(nav);
  nav.querySelectorAll<HTMLElement>("[data-account]").forEach((node) => {
    node.addEventListener("click", () => {
      const id = node.getAttribute("data-account") || null;
      state.accountFilter = id;
      renderAccounts();
      renderFolders();
      renderLabels();
      void refreshLabels();
      void refreshList();
    });
  });
}

function labelCount(id: string): number {
  return state.labels
    .filter((l) => (state.accountFilter ? l.accountId === state.accountFilter : true))
    .filter((l) => l.id === id)
    .reduce((sum, l) => sum + l.unread, 0);
}

function renderFolders(): void {
  const nav = el<HTMLDivElement>("#folders-nav");
  nav.innerHTML = FOLDERS.map((f) => {
    const count = f.id ? labelCount(f.id) : 0;
    return `
      <button class="side-item ${state.folder === f.id ? "active" : ""}" data-folder="${f.id ?? "__all__"}">
        <span class="icon">${f.icon}</span>
        <span class="name">${f.label}</span>
        ${count > 0 && f.id === "INBOX" ? `<span class="badge">${count}</span>` : ""}
      </button>`;
  }).join("");

  nav.querySelectorAll<HTMLElement>("[data-folder]").forEach((node) => {
    node.addEventListener("click", () => {
      const raw = node.getAttribute("data-folder") || "__all__";
      state.folder = raw === "__all__" ? null : raw;
      renderFolders();
      void refreshList();
    });
  });
}

function renderLabels(): void {
  const block = el<HTMLDivElement>("#labels-block");
  const nav = el<HTMLDivElement>("#labels-nav");
  const custom = state.labels.filter(
    (l) =>
      l.kind === "user" &&
      (!state.accountFilter || l.accountId === state.accountFilter) &&
      l.id,
  );
  const unique = new Map<string, Label>();
  for (const l of custom) {
    if (!unique.has(l.id)) unique.set(l.id, l);
  }
  block.style.display = unique.size ? "" : "none";
  nav.innerHTML = [...unique.values()]
    .sort((a, b) => a.name.localeCompare(b.name, "sl"))
    .map(
      (l) => `
      <button class="side-item ${state.folder === l.id ? "active" : ""}" data-label="${esc(l.id)}">
        <span class="icon" style="color:${l.color ?? "#8b93a7"}">●</span>
        <span class="name">${esc(l.name)}</span>
        ${l.unread > 0 ? `<span class="badge">${l.unread}</span>` : ""}
      </button>`,
    )
    .join("");

  nav.querySelectorAll<HTMLElement>("[data-label]").forEach((node) => {
    node.addEventListener("click", () => {
      state.folder = node.getAttribute("data-label");
      renderFolders();
      renderLabels();
      void refreshList();
    });
  });
}

function renderListHeader(): void {
  const header = el<HTMLDivElement>("#list-header");
  const scope = state.accountFilter ? accountEmail(state.accountFilter) : "vsi računi";
  header.innerHTML = `<strong>${folderLabel(state.folder)}</strong> · ${esc(scope)}`;
  el<HTMLDivElement>("#status").textContent = state.loading
    ? "Nalagam…"
    : `${state.messages.length} sporočil`;
}

function renderList(): void {
  renderListHeader();
  const list = el<HTMLDivElement>("#message-list");

  if (!state.loading && state.messages.length === 0) {
    list.innerHTML = `
      <div class="empty-state">
        <h2>Ni sporočil</h2>
        <p>${
          state.accounts.length === 0
            ? "Dodajte svoj prvi Gmail račun v nastavitvah."
            : "V tej mapi ni sporočil ali pa še ni sinhronizacije."
        }</p>
      </div>`;
    el<HTMLDivElement>("#list-footer").innerHTML = "";
    return;
  }

  list.innerHTML = state.messages
    .map((m) => {
      const sender = m.fromName || m.fromEmail;
      return `
      <div class="msg ${m.unread ? "unread" : ""} ${state.selectedId === m.id ? "selected" : ""}"
           data-account="${esc(m.accountId)}" data-id="${esc(m.id)}">
        ${avatarHtml(accountById(m.accountId), accountEmail(m.accountId), 20)}
        <div class="msg-main">
          <div class="msg-line1">
            <span class="sender">${esc(sender)}</span>
            <span class="msg-date">${fmtDate(m.date)}</span>
          </div>
          <div class="msg-subject">${esc(m.subject || "(brez zadeve)")}</div>
          <div class="msg-snippet">${esc(m.snippet)}</div>
        </div>
        <div class="msg-flags">
          <button class="icon-btn ${m.starred ? "on" : ""}" data-star="1" title="Zvezdica">★</button>
          <button class="icon-btn danger" data-delete="1" title="Izbriši">🗑</button>
          ${m.hasAttachments ? '<span class="flag">📎</span>' : ""}
        </div>
      </div>`;
    })
    .join("");

  list.querySelectorAll<HTMLElement>(".msg").forEach((node) => {
    const accountId = node.getAttribute("data-account") || "";
    const id = node.getAttribute("data-id") || "";
    node.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-star]")) {
        event.stopPropagation();
        void toggleStar(accountId, id);
        return;
      }
      if (target.closest("[data-delete]")) {
        event.stopPropagation();
        void trash({ accountId, id, threadId: "", labelIds: [], snippet: "", subject: "", fromName: "", fromEmail: "", to: "", date: 0, unread: false, starred: false, hasAttachments: false } as any);
        return;
      }
      void openMessage(accountId, id);
    });
  });
  wireAvatars(list);

  const footer = el<HTMLDivElement>("#list-footer");
  const canLoadMore =
    !state.query.trim() && state.messages.length > 0 && state.messages.length % PAGE === 0;
  footer.innerHTML = canLoadMore
    ? '<button class="btn block small" id="load-more">Naloži več</button>'
    : "";
  if (canLoadMore) {
    el<HTMLButtonElement>("#load-more").addEventListener("click", () => void refreshList(true));
  }
}

function sanitizeHtml(html: string): string {
  return html
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<base[^>]*>/gi, "")
    .replace(/\son[a-z]+\s*=\s*"[^"]*"/gi, "")
    .replace(/\son[a-z]+\s*=\s*'[^']*'/gi, "");
}

function withoutImages(html: string): string {
  return html
    .replace(/(\ssrc\s*=\s*["'])https?:/gi, "$1prevented:")
    .replace(/\ssrcset\s*=\s*("[^"]*"|'[^']*')/gi, "");
}

function renderReader(): void {
  const pane = el<HTMLDivElement>("#reader-pane");
  const message = state.selected;

  if (!message) {
    pane.innerHTML = `
      <div class="placeholder">
        <div class="ph-icon">✉</div>
        <p>Izberite sporočilo za branje</p>
      </div>`;
    return;
  }

  const sender = message.fromName
    ? `${message.fromName} <span style="color:var(--muted)">&lt;${esc(message.fromEmail)}&gt;</span>`
    : esc(message.fromEmail);
  const hasImages = !!(message.bodyHtml && /(src|background)\s*=\s*["']https?:/i.test(message.bodyHtml));

  pane.innerHTML = `
    <div class="reader-head">
      <div class="reader-subject">${esc(message.subject || "(brez zadeve)")}</div>
      <div class="reader-meta">
        ${avatarHtml(accountById(message.accountId), accountEmail(message.accountId), 16)}
        <span class="who">${sender}</span>
        <span>${fmtFull(message.date)}</span>
        <span>→ ${esc(message.to)}</span>
        <span style="color:${accountColor(message.accountId)}">${esc(accountEmail(message.accountId))}</span>
      </div>
      <div class="reader-actions">
        <button class="btn small" id="btn-reply">↩ Odgovori</button>
        <button class="btn small" id="btn-archive">🗄 Prezvrsti</button>
        <button class="btn small" id="btn-read">${message.unread ? "✉ Označi prebrano" : "✉ Označi neprebrano"}</button>
        <button class="btn small" id="btn-star">${message.starred ? "★ Odstrani zvezdico" : "☆ Zvezdica"}</button>
        <button class="btn small danger" id="btn-trash">🗑 Izbriši</button>
        ${hasImages ? '<button class="btn small" id="btn-images">🖼 Prikaži slike</button>' : ""}
      </div>
    </div>
    <div class="reader-body" id="reader-body"></div>
    ${
      message.attachments.length
        ? `<div class="attachments">${message.attachments
            .map(
              (a) => `
          <button class="attach" data-att="${esc(a.partId)}" data-name="${esc(a.filename)}" title="Prenesi v mapo Prenosi">
            📎 ${esc(a.filename)} <span style="color:var(--muted)">${fmtSize(a.size)}</span>
          </button>`,
            )
            .join("")}</div>`
        : ""
    }`;

  const body = el<HTMLDivElement>("#reader-body");
  if (message.bodyHtml) {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin");
    frame.setAttribute("referrerpolicy", "no-referrer");
    const html = sanitizeHtml(state.showImages ? message.bodyHtml : withoutImages(message.bodyHtml));
    frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8">
      <style>html,body{margin:0;padding:16px 20px;font-family:"Segoe UI",system-ui,sans-serif;
      font-size:14px;line-height:1.55;color:#1b1f24;background:#fff;word-break:break-word}
      img{max-width:100%;height:auto} table{max-width:100%} a{color:#2a6fd6}
      blockquote{border-left:3px solid #dfe3ea;margin:8px 0;padding-left:12px;color:#555}</style>
      </head><body>${html}</body></html>`;
    body.appendChild(frame);
  } else {
    const pre = document.createElement("pre");
    pre.className = "plain";
    pre.textContent = message.bodyText || "(sporočilo je brez vsebine)";
    body.appendChild(pre);
  }

  el<HTMLButtonElement>("#btn-reply").addEventListener("click", () => composeReply(message));
  el<HTMLButtonElement>("#btn-archive").addEventListener("click", () => void archive(message));
  el<HTMLButtonElement>("#btn-read").addEventListener("click", () => void toggleRead(message));
  el<HTMLButtonElement>("#btn-star").addEventListener("click", () =>
    void toggleStar(message.accountId, message.id),
  );
  el<HTMLButtonElement>("#btn-trash").addEventListener("click", () => void trash(message));
  if (hasImages) {
    el<HTMLButtonElement>("#btn-images").addEventListener("click", () => {
      state.showImages = true;
      renderReader();
    });
  }

  pane.querySelectorAll<HTMLElement>("[data-att]").forEach((node) => {
    node.addEventListener("click", () => {
      const attachmentId = node.getAttribute("data-att") || "";
      const filename = node.getAttribute("data-name") || "priponka";
      void downloadAttachment(message, attachmentId, filename);
    });
  });
  wireAvatars(pane);
}

// --------------------------------------------------------------- loading

async function refreshAccounts(): Promise<void> {
  try {
    state.accounts = await api.listAccounts();
  } catch (error) {
    toast(String(error), "error");
  }
  renderAccounts();
}

async function refreshLabels(): Promise<void> {
  try {
    state.labels = await api.listLabels(state.accountFilter);
  } catch {
    state.labels = [];
  }
  renderFolders();
  renderLabels();
}

async function refreshList(append = false): Promise<void> {
  const limit = append ? PAGE : Math.max(PAGE, state.offset);
  if (!append) {
    state.offset = 0;
  }
  state.loading = true;
  renderListHeader();

  const query = state.query.trim();
  try {
    if (query.length >= 2) {
      state.messages = await api.searchMessages(query, state.accountFilter, 60);
    } else {
      const options: ListOptions = {
        accountId: state.accountFilter,
        label: state.folder,
        query: query.length ? query : null,
        limit,
        offset: state.offset,
      };
      const page = await api.listMessages(options);
      state.messages = append ? [...state.messages, ...page] : page;
      state.offset += page.length;
    }
  } catch (error) {
    toast(String(error), "error");
  }

  state.loading = false;
  renderList();
}

async function openMessage(accountId: string, id: string): Promise<void> {
  state.selectedId = id;
  renderList();
  const pane = el<HTMLDivElement>("#reader-pane");
  pane.innerHTML = '<div class="placeholder"><span class="spinner"></span></div>';

  try {
    const message = await api.getMessage(accountId, id);
    if (state.selectedId !== id) return;
    state.selected = message;
    state.showImages = false;
    renderReader();

    if (message.unread) {
      const items = await api.setLabels(accountId, id, [], ["UNREAD"]);
      message.unread = items.includes("UNREAD");
      const summary = state.messages.find((m) => m.id === id && m.accountId === accountId);
      if (summary) summary.unread = message.unread;
      renderList();
      await refreshAccounts();
    }
  } catch (error) {
    pane.innerHTML = "";
    renderReader();
    toast(String(error), "error");
  }
}

// --------------------------------------------------------------- actions

async function toggleStar(accountId: string, id: string): Promise<void> {
  const summary = state.messages.find((m) => m.id === id && m.accountId === accountId);
  const starred = summary?.starred ?? state.selected?.starred ?? false;
  try {
    const labels = await api.setLabels(
      accountId,
      id,
      starred ? [] : ["STARRED"],
      starred ? ["STARRED"] : [],
    );
    const nowStarred = labels.includes("STARRED");
    if (summary) summary.starred = nowStarred;
    if (state.selected && state.selected.id === id) {
      state.selected.starred = nowStarred;
      renderReader();
    }
    renderList();
    await refreshLabels();
  } catch (error) {
    toast(String(error), "error");
  }
}

async function toggleRead(message: MessageFull): Promise<void> {
  try {
    const labels = await api.setLabels(
      message.accountId,
      message.id,
      message.unread ? [] : ["UNREAD"],
      message.unread ? ["UNREAD"] : [],
    );
    message.unread = labels.includes("UNREAD");
    const summary = state.messages.find(
      (m) => m.id === message.id && m.accountId === message.accountId,
    );
    if (summary) summary.unread = message.unread;
    renderReader();
    renderList();
    await refreshAccounts();
    await refreshLabels();
  } catch (error) {
    toast(String(error), "error");
  }
}

async function archive(message: MessageFull): Promise<void> {
  try {
    await api.setLabels(message.accountId, message.id, [], ["INBOX"]);
    toast("Sporočilo prezvrženo iz Prejeta.", "ok");
    state.selected = null;
    state.selectedId = null;
    renderReader();
    await refreshList();
    await refreshAccounts();
    await refreshLabels();
  } catch (error) {
    toast(String(error), "error");
  }
}

async function trash(input: { accountId: string; id: string } | MessageFull): Promise<void> {
  const accountId = ("accountId" in input && !("fromName" in input)) ? (input as { accountId: string; id: string }).accountId : input.accountId;
  const id = ("id" in input && !("fromName" in input)) ? (input as { accountId: string; id: string }).id : input.id;
  try {
    await api.trashMessage(accountId, id, true);
    toast("Sporočilo premaknjeno v Smeti.", "ok");
    state.selected = null;
    state.selectedId = null;
    renderReader();
    await refreshList();
    await refreshAccounts();
    await refreshLabels();
  } catch (error) {
    toast(String(error), "error");
  }
}

async function downloadAttachment(
  message: MessageFull,
  attachmentId: string,
  filename: string,
): Promise<void> {
  toast(`Prenašam ${filename}…`);
  try {
    const path = await api.downloadAttachment(
      message.accountId,
      message.id,
      attachmentId,
      filename,
    );
    toast(`Shranjeno: ${path}`, "ok");
  } catch (error) {
    toast(String(error), "error");
  }
}

// ---------------------------------------------------------------- modals

function closeModal(): void {
  el<HTMLDivElement>("#modal-root").innerHTML = "";
}

function openModal(html: string): HTMLElement {
  const root = el<HTMLDivElement>("#modal-root");
  root.innerHTML = `<div class="modal-backdrop"><div class="modal">${html}</div></div>`;
  const backdrop = root.firstElementChild as HTMLElement;
  backdrop.addEventListener("click", (event) => {
    if (event.target === backdrop) closeModal();
  });
  return root.querySelector(".modal") as HTMLElement;
}

interface ComposeData {
  accountId: string;
  to: string;
  cc: string;
  subject: string;
  body: string;
  threadId: string | null;
  inReplyTo: string | null;
  references: string | null;
}

async function openCompose(prefill?: Partial<ComposeData>): Promise<void> {
  if (state.accounts.length === 0) {
    toast("Najprej dodajte Gmail račun.", "error");
    await openSettings();
    return;
  }
  const defaultAccount =
    prefill?.accountId ?? state.accountFilter ?? state.accounts[0].id;

  const modal = openModal(`
    <h2>Novo sporočilo</h2>
    <p class="hint">Pošiljanje poteka prek Gmaila iz izbranega računa.</p>
    <div class="row">
      <div class="field">
        <label>Iz računa</label>
        <div class="from-row">
          <span id="c-avatar">${avatarHtml(accountById(defaultAccount), "", 22)}</span>
          <select id="c-from">
            ${state.accounts
              .map(
                (a) =>
                  `<option value="${esc(a.id)}" ${a.id === defaultAccount ? "selected" : ""}>${esc(a.email)}</option>`,
              )
              .join("")}
          </select>
        </div>
      </div>
      <div class="field">
        <label>Za</label>
        <input id="c-to" placeholder="ime@primer.si" value="${esc(prefill?.to ?? "")}" />
      </div>
    </div>
    <div class="row">
      <div class="field">
        <label>Kp (cc)</label>
        <input id="c-cc" value="${esc(prefill?.cc ?? "")}" />
      </div>
      <div class="field">
        <label>Skp (bcc)</label>
        <input id="c-bcc" />
      </div>
    </div>
    <div class="field">
      <label>Zadeva</label>
      <input id="c-subject" value="${esc(prefill?.subject ?? "")}" />
    </div>
    <div class="field">
      <label>Sporočilo</label>
      <textarea id="c-body">${esc(prefill?.body ?? "")}</textarea>
    </div>
    <div class="modal-actions">
      <button class="btn" id="c-cancel">Prekliči</button>
      <button class="btn primary" id="c-send">Pošlji</button>
    </div>
  `);

  modal.querySelector<HTMLButtonElement>("#c-cancel")?.addEventListener("click", closeModal);

  const fromSelect = modal.querySelector<HTMLSelectElement>("#c-from");
  const paintFromAvatar = () => {
    const holder = modal.querySelector<HTMLSpanElement>("#c-avatar");
    if (!holder || !fromSelect) return;
    const account = accountById(fromSelect.value);
    holder.innerHTML = avatarHtml(account, account?.email ?? "", 22);
    wireAvatars(holder);
  };
  fromSelect?.addEventListener("change", paintFromAvatar);
  wireAvatars(modal);

  const sendButton = modal.querySelector<HTMLButtonElement>("#c-send");
  sendButton?.addEventListener("click", async () => {
    const from = modal.querySelector<HTMLSelectElement>("#c-from")?.value || "";
    const to = modal.querySelector<HTMLInputElement>("#c-to")?.value.trim() || "";
    const cc = modal.querySelector<HTMLInputElement>("#c-cc")?.value.trim() || "";
    const bcc = modal.querySelector<HTMLInputElement>("#c-bcc")?.value.trim() || "";
    const subject = modal.querySelector<HTMLInputElement>("#c-subject")?.value || "";
    const body = modal.querySelector<HTMLTextAreaElement>("#c-body")?.value || "";

    if (!to) {
      toast("Vnesite prejemnika.", "error");
      return;
    }

    const input: SendInput = {
      accountId: from,
      to,
      cc: cc || null,
      bcc: bcc || null,
      subject,
      bodyText: body,
      threadId: prefill?.threadId ?? null,
      inReplyTo: prefill?.inReplyTo ?? null,
      references: prefill?.references ?? null,
    };

    sendButton.disabled = true;
    sendButton.innerHTML = '<span class="spinner"></span> Pošiljam…';
    try {
      await api.sendMessage(input);
      toast("Sporočilo poslano.", "ok");
      closeModal();
      await refreshList();
    } catch (error) {
      toast(String(error), "error");
      sendButton.disabled = false;
      sendButton.textContent = "Pošlji";
    }
  });
}

async function composeReply(message: MessageFull): Promise<void> {
  const to = message.replyTo || message.fromEmail;
  const subject = /^re:/i.test(message.subject) ? message.subject : `Re: ${message.subject}`;
  const quoted = (message.bodyText || message.snippet || "")
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");
  const references = [message.references, message.rfcMessageId].filter(Boolean).join(" ");

  await openCompose({
    accountId: message.accountId,
    to,
    subject,
    body: `\n\n--- ${fmtFull(message.date)} je ${message.fromName || message.fromEmail} napisal(a):\n${quoted}`,
    threadId: message.threadId,
    inReplyTo: message.rfcMessageId,
    references: references || null,
  });
}

async function openSettings(): Promise<void> {
  const modal = openModal(`
    <h2>Nastavitve</h2>
    <p class="hint">MailHub uporablja lasten Google OAuth odjemalec. Ustvarite ga enkrat v Google Cloud Console (brezplačno) in prilepite podatke spodaj.</p>
    <ol class="steps">
      <li>Odprite <a id="link-console">Google Cloud Console → APIs &amp; Services → Credentials</a>.</li>
      <li>Ustvarite projekt, omogočite <b>Gmail API</b>.</li>
      <li>Credentials → Create credentials → <b>OAuth client ID</b> → tip <b>Desktop app</b>.</li>
      <li>Prekopirajte <b>Client ID</b> in <b>Client secret</b> (oba sta obvezna – Google secret zahteva tudi pri tipu Desktop app).</li>
      <li>OAuth consent screen: dodajte svoj e-naslov med <b>Test users</b>.</li>
    </ol>
    <div class="field">
      <label>Client ID</label>
      <input id="s-client-id" placeholder="xxxxx.apps.googleusercontent.com" />
    </div>
    <div class="field">
      <label>Client secret (obvezno)</label>
      <input id="s-client-secret" type="password" placeholder="GOCSPX-…" />
    </div>
    <div class="modal-actions" style="justify-content:space-between">
      <button class="btn" id="s-save">Shrani in preveri</button>
      <button class="btn" id="s-close">Zapri</button>
    </div>
    <div id="s-setup" class="hint" style="margin-top:8px"></div>
    <h2 style="margin-top:22px">Računi</h2>
    <div id="s-accounts"></div>
    <div class="quick-add" id="s-quick"></div>
    <div class="field" style="margin-top:14px">
      <label>Dodaj račun (neobvezno: namig za e-naslov)</label>
      <div style="display:flex;gap:8px">
        <input id="s-hint" placeholder="ime@gmail.com" />
        <button class="btn primary" id="s-add">Dodaj račun</button>
      </div>
    </div>
    <div id="s-status" class="hint" style="margin-top:10px"></div>
    <details id="s-manual" style="margin-top:12px">
      <summary class="hint" style="cursor:pointer">Ročna dokončitev prijave (če se brskalnik ne vrne sam)</summary>
      <p class="hint" style="margin:8px 0">
        Če po prijavi v brskalniku ostaneš na prazni strani ali se ne vrneš v MailHub, prekopiraj
        celoten naslov iz naslovne vrstice (začne se s <code>http://127.0.0.1:…/?code=…</code>) in
        ga prilepi spodaj.
      </p>
      <div style="display:flex;gap:8px">
        <input id="s-code-url" placeholder="http://127.0.0.1:…/?code=…&amp;state=…" />
        <button class="btn" id="s-code-submit">Dokončaj prijavo</button>
      </div>
      <div style="margin-top:8px">
        <button class="btn small ghost" id="s-cancel">Prekliči čakanje na prijavo</button>
      </div>
    </details>
    <div class="modal-actions" style="justify-content:flex-start;margin-top:10px">
      <button class="btn small ghost" id="s-log">📄 Odpri dnevnik</button>
    </div>
    <div class="field" style="margin-top:14px">
      <label style="display:flex;align-items:center;gap:8px;cursor:pointer">
        <input type="checkbox" id="s-autostart" />
        <span>Zaženi MailHub ob prijavi v Windows (minimiran v zaledju)</span>
      </label>
    </div>
    <div id="s-autostart-status" class="hint" style="margin-top:4px"></div>
  `);

  const clientIdInput = modal.querySelector<HTMLInputElement>("#s-client-id");
  const secretInput = modal.querySelector<HTMLInputElement>("#s-client-secret");
  const status = modal.querySelector<HTMLDivElement>("#s-status");
  const setupNote = modal.querySelector<HTMLDivElement>("#s-setup");

  void api.setupStatus().then((statusInfo) => {
    if (clientIdInput && statusInfo.clientId) clientIdInput.value = statusInfo.clientId;
    if (setupNote) {
      if (!statusInfo.hasOauthClient) {
        setupNote.innerHTML =
          '<span style="color:#ffb3b3">OAuth odjemalec še ni shranjen – brez njega prijava ne deluje.</span>';
      } else if (!statusInfo.hasSecret) {
        setupNote.innerHTML =
          '<span style="color:#ffb3b3">Manjka Client secret – Google bo prijavo zavrnil. ' +
          "Prilepi ga in klikni »Shrani in preveri«.</span>";
      } else {
        setupNote.textContent = "OAuth odjemalec je shranjen (Client ID + secret).";
      }
    }
  });

  modal.querySelector("#link-console")?.addEventListener("click", () => {
    void api.openPath("https://console.cloud.google.com/apis/credentials");
  });

  modal.querySelector<HTMLButtonElement>("#s-log")?.addEventListener("click", async () => {
    try {
      await api.openLog();
    } catch (error) {
      toast(String(error), "error");
    }
  });

  modal.querySelector<HTMLButtonElement>("#s-save")?.addEventListener("click", async (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    button.disabled = true;
    button.innerHTML = '<span class="spinner"></span> Preverjam…';
    try {
      const warning = await api.setOauthClient(
        clientIdInput?.value ?? "",
        secretInput?.value.trim() || undefined,
      );
      state.hasClient = true;
      state.hasSecret = true;
      if (warning) {
        toast(warning, "error");
        if (setupNote) setupNote.textContent = warning;
      } else {
        toast("OAuth odjemalec je shranjen in preverjen pri Googlu.", "ok");
        if (setupNote) setupNote.textContent = "OAuth odjemalec je shranjen in preverjen pri Googlu.";
      }
      renderQuickAdd(modal, status);
    } catch (error) {
      toast(String(error), "error");
      if (setupNote) setupNote.textContent = String(error);
    } finally {
      button.disabled = false;
      button.textContent = "Shrani in preveri";
    }
  });

  modal.querySelector<HTMLButtonElement>("#s-code-submit")?.addEventListener("click", async (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    const input = modal.querySelector<HTMLInputElement>("#s-code-url");
    const value = input?.value.trim() ?? "";
    if (!value) {
      toast("Prilepi celoten naslov iz brskalnika.", "error");
      return;
    }
    button.disabled = true;
    try {
      await api.submitAuthUrl(value);
      toast("Prijava prejeta – dokončujem povezavo računa.", "ok");
      if (status) status.textContent = "Prijava prejeta – dokončujem povezavo računa…";
      if (input) input.value = "";
    } catch (error) {
      toast(String(error), "error");
      if (status) status.textContent = String(error);
    } finally {
      button.disabled = false;
    }
  });

  modal.querySelector<HTMLButtonElement>("#s-cancel")?.addEventListener("click", async () => {
    try {
      await api.cancelLogin();
      if (status) status.textContent = "Čakanje na prijavo preklicano. Lahko poskusiš znova.";
      toast("Čakanje na prijavo preklicano.", "ok");
    } catch (error) {
      toast(String(error), "error");
    }
  });

  modal.querySelector<HTMLButtonElement>("#s-close")?.addEventListener("click", closeModal);

  // Autostart checkbox
  const autostartCheckbox = modal.querySelector<HTMLInputElement>("#s-autostart");
  const autostartStatus = modal.querySelector<HTMLDivElement>("#s-autostart-status");
  async function refreshAutostartStatus(): Promise<void> {
    try {
      const enabled = await api.autostartIsEnabled();
      if (autostartCheckbox) autostartCheckbox.checked = enabled;
      if (autostartStatus) {
        autostartStatus.textContent = enabled
          ? "✓ MailHub se bo zaganzal ob prijavi v Windows."
          : "MailHub se ne bo zaganzal sam.";
      }
    } catch (error) {
      if (autostartStatus) autostartStatus.textContent = `Napaka: ${error}`;
    }
  }
  await refreshAutostartStatus();

  autostartCheckbox?.addEventListener("change", async () => {
    if (!autostartCheckbox) return;
    try {
      if (autostartCheckbox.checked) {
        await api.autostartEnable();
        toast("Avtomatski zagon omogočen.", "ok");
      } else {
        await api.autostartDisable();
        toast("Avtomatski zagon onemogočen.", "ok");
      }
      await refreshAutostartStatus();
    } catch (error) {
      toast(String(error), "error");
      // Revert checkbox on error
      autostartCheckbox.checked = !autostartCheckbox.checked;
      await refreshAutostartStatus();
    }
  });

  const addButton = modal.querySelector<HTMLButtonElement>("#s-add");
  addButton?.addEventListener("click", () => {
    const hint = modal.querySelector<HTMLInputElement>("#s-hint")?.value.trim() || undefined;
    void runAddAccount(addButton, hint, status, modal);
  });

  renderSettingsAccounts(modal);
}

let loginBusy = false;

const QUICK_ACCOUNTS = [
  "janistimac@gmail.com",
  "jbj08.f8g@gmail.com",
  "sodelovanje.jbj08@gmail.com",
];

async function runAddAccount(
  button: HTMLButtonElement,
  hint: string | undefined,
  status: HTMLElement | null,
  modal: HTMLElement,
): Promise<void> {
  if (!state.hasClient || !state.hasSecret) {
    toast(
      "Najprej vnesi Client ID in Client secret v Nastavitvah ter klikni »Shrani in preveri«.",
      "error",
    );
    const secret = modal.querySelector<HTMLInputElement>("#s-client-secret");
    secret?.focus();
    if (status) {
      status.textContent =
        "Manjka Client secret – brez njega Google prijave ne dovoli. Prilepi ga zgoraj in shrani.";
    }
    return;
  }
  if (loginBusy) {
    toast(
      "Prijava je že v teku. Dokončaj jo v brskalniku ali prilepi povezavo pod »Ročna dokončitev prijave«.",
      "error",
    );
    return;
  }
  const label = button.dataset.label ?? button.textContent ?? "Dodaj račun";
  button.dataset.label = label;
  button.disabled = true;
  button.innerHTML = '<span class="spinner"></span> Čakam…';
  loginBusy = true;
  if (status) {
    status.textContent =
      "Odprl se je brskalnik. Prijavi se v Google in dovoli dostop. Če po prijavi ne prideš nazaj " +
      "v MailHub (prazna stran ali napaka), prekopiraj naslov iz brskalnika in ga prilepi pod " +
      "»Ročna dokončitev prijave«.";
  }
  try {
    const pendingAccount = api.addAccount(hint);
    const helpTimer = window.setTimeout(() => {
      void (async () => {
        try {
          const url = await api.loginUrl();
          if (url) renderLoginHelp(modal, url, status);
        } catch {
          /* pomozna ponudba ni nujna */
        }
      })();
    }, 1500);
    let account;
    try {
      account = await pendingAccount;
    } finally {
      window.clearTimeout(helpTimer);
      modal.querySelector("#s-login-help")?.remove();
    }
    toast(`Račun ${account.email} dodan. Sinhroniziram…`, "ok");
    if (status) status.textContent = `Račun ${account.email} je povezan.`;
    await refreshAccounts();
    await refreshLabels();
    await refreshList();
    renderSettingsAccounts(modal);
    renderQuickAdd(modal, status);
  } catch (error) {
    toast(String(error), "error");
    if (status) status.textContent = String(error);
  } finally {
    loginBusy = false;
    button.disabled = false;
    button.textContent = label;
  }
}

function renderLoginHelp(modal: HTMLElement, url: string, status: HTMLElement | null): void {
  modal.querySelector("#s-login-help")?.remove();
  const box = document.createElement("div");
  box.id = "s-login-help";
  box.className = "hint";
  box.style.marginTop = "10px";
  box.innerHTML =
    '<div style="margin-bottom:6px">Brskalnik se ne vrne sam? Povezavo lahko odpreš tudi ročno:</div>' +
    `<input readonly id="s-login-url" value="${esc(url)}" style="width:100%" />` +
    '<div style="display:flex;gap:8px;margin-top:6px">' +
    '<button class="btn small" id="s-login-open">Odpri v brskalniku</button>' +
    '<button class="btn small ghost" id="s-login-copy">Kopiraj povezavo</button>' +
    "</div>";
  if (status?.parentElement) {
    status.parentElement.insertBefore(box, status.nextSibling);
  } else {
    modal.querySelector(".modal-actions")?.before(box);
  }

  box.querySelector<HTMLButtonElement>("#s-login-open")?.addEventListener("click", () => {
    void api.openPath(url);
  });
  box.querySelector<HTMLButtonElement>("#s-login-copy")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(url);
      toast("Prijavna povezava kopirana.", "ok");
    } catch {
      const input = box.querySelector<HTMLInputElement>("#s-login-url");
      input?.select();
      toast("Kopiranje ni uspelo – besedilo je označeno, pritisni Ctrl+C.", "error");
    }
  });
}

function renderQuickAdd(modal: HTMLElement, status: HTMLElement | null): void {
  const box = modal.querySelector<HTMLDivElement>("#s-quick");
  if (!box) return;
  const existing = new Set(state.accounts.map((a) => a.email.toLowerCase()));
  const missing = QUICK_ACCOUNTS.filter((email) => !existing.has(email));
  if (missing.length === 0) {
    box.innerHTML = "";
    return;
  }
  box.innerHTML =
    '<div class="hint" style="margin:16px 0 6px">Tvoji računi – klikni za povezavo:</div>' +
    missing
      .map(
        (email) =>
          `<button class="btn small" data-quick="${esc(email)}">＋ ${esc(email)}</button>`,
      )
      .join(" ");
  box.querySelectorAll<HTMLButtonElement>("[data-quick]").forEach((node) => {
    node.addEventListener("click", () => {
      const email = node.getAttribute("data-quick") ?? undefined;
      void runAddAccount(node, email, status, modal);
    });
  });
}

function renderSettingsAccounts(modal: HTMLElement): void {
  const container = modal.querySelector<HTMLDivElement>("#s-accounts");
  if (!container) return;

  if (state.accounts.length === 0) {
    container.innerHTML = '<p class="hint">Ni povezanih računov.</p>';
    return;
  }

  container.innerHTML = state.accounts
    .map(
      (a) => `
      <div class="account-row" data-account="${esc(a.id)}">
        ${avatarHtml(a, a.email, 22)}
        <span class="mail">${esc(a.email)}</span>
        <span class="state ${a.syncError ? "err" : ""}">
          ${
            a.syncError
              ? esc(a.syncError.slice(0, 60))
              : a.lastSync
                ? `sinhronizirano ${fmtDate(a.lastSync)}`
                : "še ni sinhronizirano"
          }
        </span>
        <button class="btn small danger" data-remove="${esc(a.id)}">Odstrani</button>
      </div>`,
    )
    .join("");

  container.querySelectorAll<HTMLElement>("[data-remove]").forEach((node) => {
    let armed = false;
    node.addEventListener("click", async () => {
      const id = node.getAttribute("data-remove") || "";
      if (!armed) {
        armed = true;
        node.textContent = "Potrdi?";
        setTimeout(() => {
          if (armed) {
            armed = false;
            node.textContent = "Odstrani";
          }
        }, 3500);
        return;
      }
      armed = false;
      try {
        await api.removeAccount(id);
        if (state.accountFilter === id) state.accountFilter = null;
        toast("Račun odstranjen. Pošta v Gmailu ostane nedotaknjena.", "ok");
        await refreshAccounts();
        await refreshLabels();
        await refreshList();
        renderSettingsAccounts(modal);
      } catch (error) {
        toast(String(error), "error");
      }
    });
  });

  renderQuickAdd(modal, modal.querySelector<HTMLDivElement>("#s-status"));
  wireAvatars(container);
}

// ------------------------------------------------------------------- boot

function debounce(fn: () => void, delay: number): () => void {
  let handle: number | undefined;
  return () => {
    if (handle !== undefined) window.clearTimeout(handle);
    handle = window.setTimeout(fn, delay);
  };
}

async function syncAll(): Promise<void> {
  const button = el<HTMLButtonElement>("#btn-sync");
  button.disabled = true;
  button.innerHTML = '<span class="spinner"></span> Osvežujem…';
  try {
    await api.syncAll();
    toast("Sinhronizacija se je začela.", "ok");
    setTimeout(() => void refreshAccounts(), 1500);
  } catch (error) {
    toast(String(error), "error");
  } finally {
    button.disabled = false;
    button.textContent = "⟳ Osveži vse";
  }
}

async function boot(): Promise<void> {
  el<HTMLButtonElement>("#btn-compose").addEventListener("click", () => openCompose());
  el<HTMLButtonElement>("#btn-settings").addEventListener("click", openSettings);
  el<HTMLButtonElement>("#btn-sync").addEventListener("click", () => void syncAll());

  const search = el<HTMLInputElement>("#search");
  const runSearch = debounce(() => {
    state.query = search.value;
    void refreshList();
  }, 450);
  search.addEventListener("input", runSearch);
  search.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      search.value = "";
      state.query = "";
      void refreshList();
    }
  });

  await listen<{ accountId: string }>("mailhub:changed", () => {
    const list = el<HTMLDivElement>("#message-list");
    const scroll = list.scrollTop;
    void (async () => {
      await refreshAccounts();
      await refreshLabels();
      await refreshList();
      el<HTMLDivElement>("#message-list").scrollTop = scroll;
    })();
  });

  try {
    const status = await api.setupStatus();
    state.hasClient = status.hasOauthClient;
    state.hasSecret = status.hasSecret;
  } catch (error) {
    toast(String(error), "error");
  }

  await refreshAccounts();
  await refreshLabels();
  await refreshList();
  renderReader();

  if (!state.hasClient || !state.hasSecret) {
    await openSettings();
    if (!state.hasClient) {
      toast("Dodaj Google OAuth Client ID in Client secret (Nastavitve).", "error");
    } else {
      toast("Manjka Client secret – prijava brez njega ne deluje.", "error");
    }
  } else if (state.accounts.length === 0) {
    await openSettings();
  }
}

window.addEventListener("DOMContentLoaded", () => {
  void boot();
});
