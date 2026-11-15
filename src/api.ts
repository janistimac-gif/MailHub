import { invoke } from "@tauri-apps/api/core";

export interface Account {
  id: string;
  email: string;
  name: string;
  picture: string | null;
  color: string;
  lastSync: number | null;
  syncError: string | null;
  unread: number;
}

export interface Label {
  accountId: string;
  id: string;
  name: string;
  kind: string;
  color: string | null;
  unread: number;
  total: number;
}

export interface MessageSummary {
  accountId: string;
  id: string;
  threadId: string;
  labelIds: string[];
  snippet: string;
  subject: string;
  fromName: string;
  fromEmail: string;
  to: string;
  date: number;
  unread: boolean;
  starred: boolean;
  hasAttachments: boolean;
}

export interface AttachmentMeta {
  partId: string;
  filename: string;
  mimeType: string;
  size: number;
}

export interface MessageFull extends MessageSummary {
  bodyText: string | null;
  bodyHtml: string | null;
  attachments: AttachmentMeta[];
  references: string | null;
  rfcMessageId: string | null;
  replyTo: string | null;
}

export interface SendInput {
  accountId: string;
  to: string;
  cc?: string | null;
  bcc?: string | null;
  subject: string;
  bodyText: string;
  bodyHtml?: string | null;
  threadId?: string | null;
  inReplyTo?: string | null;
  references?: string | null;
}

export interface SetupStatus {
  hasOauthClient: boolean;
  clientId: string | null;
  hasSecret: boolean;
}

export interface SyncReport {
  accountId: string;
  fetched: number;
  removed: number;
  error: string | null;
}

export interface ListOptions {
  accountId?: string | null;
  label?: string | null;
  query?: string | null;
  unreadOnly?: boolean;
  starredOnly?: boolean;
  limit?: number;
  offset?: number;
}

export const api = {
  setupStatus: () => invoke<SetupStatus>("setup_status"),
  setOauthClient: (clientId: string, clientSecret?: string) =>
    invoke<string | null>("set_oauth_client", { clientId, clientSecret: clientSecret ?? null }),
  listAccounts: () => invoke<Account[]>("list_accounts"),
  addAccount: (loginHint?: string) =>
    invoke<Account>("add_account", { loginHint: loginHint ?? null }),
  submitAuthUrl: (url: string) => invoke<void>("submit_auth_url", { url }),
  cancelLogin: () => invoke<void>("cancel_login"),
  loginUrl: () => invoke<string | null>("login_url"),
  removeAccount: (accountId: string) => invoke<void>("remove_account", { accountId }),
  syncAll: () => invoke<void>("sync_all"),
  syncAccount: (accountId: string) => invoke<SyncReport>("sync_account", { accountId }),
  listMessages: (opts: ListOptions) =>
    invoke<MessageSummary[]>("list_messages", { ...opts } as Record<string, unknown>),
  listLabels: (accountId?: string | null) =>
    invoke<Label[]>("list_labels", { accountId: accountId ?? null }),
  getMessage: (accountId: string, id: string) =>
    invoke<MessageFull>("get_message", { accountId, id }),
  searchMessages: (query: string, accountId?: string | null, limit?: number) =>
    invoke<MessageSummary[]>("search_messages", {
      query,
      accountId: accountId ?? null,
      limit: limit ?? 40,
    }),
  setLabels: (accountId: string, id: string, add: string[], remove: string[]) =>
    invoke<string[]>("set_labels", { accountId, id, add, remove }),
  trashMessage: (accountId: string, id: string, del: boolean) =>
    invoke<void>("trash_message", { accountId, id, delete: del }),
  sendMessage: (input: SendInput) => invoke<string>("send_message", { input }),
  downloadAttachment: (
    accountId: string,
    messageId: string,
    attachmentId: string,
    filename: string,
  ) =>
    invoke<string>("download_attachment", {
      accountId,
      messageId,
      attachmentId,
      filename,
    }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openLog: () => invoke<void>("open_log"),
  autostartEnable: () => invoke<void>("autostart_enable"),
  autostartDisable: () => invoke<void>("autostart_disable"),
  autostartIsEnabled: () => invoke<boolean>("autostart_is_enabled"),
};
