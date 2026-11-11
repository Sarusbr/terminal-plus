// Tabs, split-pane layout trees and the SSH connect flow.
import { reactive, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import { api, uid, type ShellKind } from "../lib/api";
import { settings } from "./settings";
import { proxyFor } from "./vpn";
import type { Server } from "./servers";
import { confirmHostKey, prompt, ui } from "./ui";
import type { RunOpts, RunResult } from "../lib/runner";

export type PaneNode =
  | { kind: "leaf"; id: string }
  | { kind: "split"; id: string; dir: "row" | "col"; ratio: number; a: PaneNode; b: PaneNode };

export interface PaneState {
  /** Alternate screen active (vim, htop, less …). Panels are disabled while true. */
  alt: boolean;
  /** Local only: a foreground program other than the shell owns the tty. */
  busy?: boolean;
  ptyId?: string;
  /** Working directory reported by the shell prompt (OSC 9;9 / OSC 7). */
  cwd?: string;
}

export type TabStatus = "connecting" | "connected" | "disconnected" | "ended" | "error";

export interface Tab {
  id: string;
  /** "local" = shell on this computer, "ssh" = remote server. */
  kind: "local" | "ssh";
  /** Local shell id ("cmd", "powershell", …) and what kind of shell it turned out to be. */
  shell?: string;
  shellKind?: ShellKind;
  server: Server;
  connId: string;
  status: TabStatus;
  /** What a connecting tab is waiting for (e.g. its VPN). */
  phase?: string;
  ip?: string;
  error?: string;
  layout: PaneNode;
  panes: Record<string, PaneState>;
  activePane: string;
  /** Bumped on every successful (re)connect; panes reopen their shells when it changes. */
  gen: number;
}

/** Imperative handles registered by mounted TerminalPane components. */
export interface PaneHandle {
  focus(): void;
  copy(): Promise<void>;
  paste(): Promise<void>;
  selectAll(): void;
  hasSelection(): boolean;
  clear(): void;
  search(): void;
  write(data: string): void;
  run(cmd: string, opts?: RunOpts): Promise<RunResult>;
}
export const paneHandles = new Map<string, PaneHandle>();

export const tabs = reactive<{ list: Tab[]; active: string }>({ list: [], active: "" });

export const activeTab = computed(() => tabs.list.find((t) => t.id === tabs.active));

export function tabLabel(t: Tab) {
  if (t.kind === "local") return `${t.server.name} · local`;
  const isIp = /^[\d.]+$|:/.test(t.server.host);
  const left = isIp ? t.server.name || t.server.host : t.server.host;
  const ip = t.ip ?? (isIp ? t.server.host : "");
  return ip && ip !== left ? `${left} · ${ip}` : left;
}

const leaf = (): PaneNode => ({ kind: "leaf", id: uid() });

export const localShells = reactive<{ list: { id: string; name: string }[]; user: string }>({ list: [], user: "" });

export async function loadLocalShells() {
  const [list, info] = await Promise.all([
    api.localShells().catch(() => []),
    api.localInfo().catch(() => ({ shell: "shell", user: "" })),
  ]);
  localShells.list = list.length ? list : [{ id: "default", name: info.shell }];
  localShells.user = info.user;
}

/** Open a shell on this computer (no SSH). `shellId` from the "+" menu; default from settings. */
export async function openLocal(shellId?: string) {
  if (!localShells.list.length) await loadLocalShells();
  const sh = localShells.list.find((s) => s.id === (shellId ?? settings.localShell)) ?? localShells.list[0];
  const root = leaf();
  const tab: Tab = reactive({
    id: uid(), kind: "local" as const, shell: sh.id,
    server: { id: "local", name: sh.name, host: "localhost", port: 0, user: localShells.user, auth: "key" as const, tags: [] },
    connId: "", status: "connected" as const,
    layout: root, panes: { [root.id]: { alt: false } }, activePane: root.id, gen: 1,
  }) as Tab;
  tabs.list.push(tab);
  tabs.active = tab.id;
  ui.page = "terminal";
}

export function openServer(server: Server) {
  const root = leaf();
  const tab: Tab = reactive({
    id: uid(), kind: "ssh" as const, server: { ...server }, connId: "", status: "connecting",
    layout: root, panes: { [root.id]: { alt: false } }, activePane: root.id, gen: 0,
  }) as Tab;
  tabs.list.push(tab);
  tabs.active = tab.id;
  ui.page = "terminal";
  connect(tab);
}

export async function connect(tab: Tab, creds: { password?: string; passphrase?: string; save?: boolean } = {}) {
  if (tab.kind === "local") {
    tab.status = "connected";
    tab.gen++;
    return;
  }
  if (tab.connId) api.disconnect(tab.connId).catch(() => {});
  const connId = uid();
  tab.connId = connId;
  tab.status = "connecting";
  tab.error = undefined;
  const s = tab.server;
  try {
    // Linked VPN: bring it up first; app mode → SSH dials through its SOCKS proxy.
    let proxy: string | undefined;
    if (s.vpnId) {
      tab.phase = "VPN bağlanıyor…";
      try {
        proxy = await proxyFor(s.vpnId);
        tab.phase = "";
      } catch (e) {
        if (tab.connId !== connId) return;
        tab.phase = "";
        tab.status = "error";
        tab.error = `VPN: ${String((e as Error)?.message ?? e)}`;
        return;
      }
    }
    const r = await api.connect({
      connId, host: s.host, port: s.port, user: s.user, auth: s.auth, proxy,
      keyPath: s.keyPath, serverId: s.id,
      password: creds.password, passphrase: creds.passphrase, saveSecret: creds.save,
    });
    if (tab.connId !== connId) return; // superseded
    tab.ip = r.ip;
    tab.status = "connected";
    tab.gen++;
  } catch (e) {
    if (tab.connId !== connId) return;
    const msg = String(e);
    if (msg.startsWith("NEED_PASSWORD") || (msg === "AUTH_FAILED" && s.auth === "password")) {
      const r = await prompt({
        title: `${s.user}@${s.host}`,
        message: msg.includes(":") || msg === "AUTH_FAILED" ? "Şifre reddedildi" : undefined,
        placeholder: "şifre", secret: true, remember: true,
      });
      if (r) return connect(tab, { password: r.value, save: r.remember });
    } else if (msg.startsWith("NEED_PASSPHRASE")) {
      const r = await prompt({
        title: "SSH key passphrase",
        message: msg.includes("wrong") ? "Passphrase hatalı" : s.keyPath,
        placeholder: "passphrase", secret: true, remember: true,
      });
      if (r) return connect(tab, { passphrase: r.value, save: r.remember });
    }
    tab.status = "error";
    tab.error = msg === "HOSTKEY_REJECTED" ? "Host key reddedildi" : msg;
  }
}

export function closeTab(id: string) {
  const i = tabs.list.findIndex((t) => t.id === id);
  if (i < 0) return;
  const [t] = tabs.list.splice(i, 1);
  if (t.connId) api.disconnect(t.connId).catch(() => {});
  if (tabs.active === id) tabs.active = tabs.list[Math.min(i, tabs.list.length - 1)]?.id ?? "";
}

export function moveTab(from: number, to: number) {
  if (from === to) return;
  const [t] = tabs.list.splice(from, 1);
  tabs.list.splice(to, 0, t);
}

export function splitPane(tab: Tab, paneId: string, dir: "row" | "col") {
  const fresh = leaf();
  const replace = (n: PaneNode): PaneNode => {
    if (n.kind === "leaf") {
      return n.id === paneId ? { kind: "split", id: uid(), dir, ratio: 0.5, a: n, b: fresh } : n;
    }
    return { ...n, a: replace(n.a), b: replace(n.b) };
  };
  tab.panes[fresh.id] = { alt: false };
  tab.layout = replace(tab.layout);
  tab.activePane = fresh.id;
}

export function closePane(tab: Tab, paneId: string) {
  if (tab.layout.kind === "leaf") return closeTab(tab.id);
  const remove = (n: PaneNode): PaneNode | null => {
    if (n.kind === "leaf") return n.id === paneId ? null : n;
    const a = remove(n.a), b = remove(n.b);
    if (!a) return b;
    if (!b) return a;
    return { ...n, a, b };
  };
  tab.layout = remove(tab.layout)!;
  delete tab.panes[paneId];
  if (tab.activePane === paneId) {
    let first: PaneNode = tab.layout;
    while (first.kind === "split") first = first.a;
    tab.activePane = first.id;
  }
  paneHandles.get(tab.activePane)?.focus();
}

function findByPty(ptyId: string) {
  for (const t of tabs.list) {
    for (const [paneId, p] of Object.entries(t.panes)) if (p.ptyId === ptyId) return { t, paneId };
  }
}

export async function initTabEvents() {
  await listen<{ connId: string; host: string; port: number; algorithm: string; fingerprint: string }>(
    "ssh-hostkey",
    async (e) => {
      const ok = await confirmHostKey(e.payload);
      api.hostkeyRespond(e.payload.connId, ok);
    },
  );
  await listen<string>("ssh-disconnected", (e) => {
    const t = tabs.list.find((x) => x.connId === e.payload);
    if (t && t.status === "connected") t.status = "disconnected";
  });
  await listen<{ ptyId: string; code: number | null }>("pty-exit", (e) => {
    const hit = findByPty(e.payload.ptyId);
    if (!hit) return;
    const { t, paneId } = hit;
    t.panes[paneId].ptyId = undefined;
    if (t.status !== "connected") return;
    if (t.kind === "local") {
      closePane(t, paneId); // shell exited → close pane (last pane closes the tab)
    } else if (e.payload.code === null) {
      // Channel vanished without exit status → connection dropped.
      t.status = "disconnected";
    } else if (t.layout.kind === "leaf") {
      t.status = "ended"; // user typed `exit` in the last pane
    } else {
      closePane(t, paneId);
    }
  });
}
