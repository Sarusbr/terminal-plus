// VPN profiles (non-secret fields in vpn.json; passwords in the OS keychain), live
// connection status, prompts (MFA, gateway, certificate) and component downloads.
import { reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { api, uid } from "../lib/api";
import { platform } from "./settings";
import { confirm, prompt, choose, toast, ui } from "./ui";

export type VpnProtocol = "openconnect" | "wireguard" | "openvpn";
export type OcProtocol = "gp" | "anyconnect" | "fortinet" | "pulse" | "f5" | "array";
export type VpnMode = "app" | "global";

export interface VpnProfile {
  id: string;
  name: string;
  protocol: VpnProtocol;
  ocProtocol: OcProtocol;
  server: string;
  user: string;
  group?: string;
  certPath?: string;
  configPath?: string;
  /** Server certificate pin the user accepted ("pin-sha256:…"). */
  servercert?: string;
  defaultMode: VpnMode;
}

export interface VpnStatus {
  id: string; name: string;
  state: "connecting" | "connected" | "reconnecting" | "disconnected" | "error";
  mode: VpnMode; ip?: string; socksPort?: number; since?: number; rx: number; tx: number;
  error?: string; errorKind?: "auth" | "cert" | "hip" | "denied" | "component" | "network" | "other"; pin?: string;
}

export const OC_PROTOCOLS: { id: OcProtocol; name: string }[] = [
  { id: "gp", name: "Palo Alto GlobalProtect" },
  { id: "anyconnect", name: "Cisco AnyConnect" },
  { id: "fortinet", name: "Fortinet SSL VPN" },
  { id: "pulse", name: "Ivanti / Pulse Secure" },
  { id: "f5", name: "F5" },
  { id: "array", name: "Array" },
];

/** App-only mode needs a userspace proxy; not offered on Windows. */
export const appModeSupported = () => platform.os !== "windows";

export const vpn = reactive({
  profiles: [] as VpnProfile[],
  status: {} as Record<string, VpnStatus>,
  download: null as null | { component: string; done: number; total: number },
  component: "unknown" as "unknown" | "downloaded" | "system" | "missing",
});

const persist = () => api.storeSave("vpn", vpn.profiles);

export async function loadVpn() {
  const data = await api.storeLoad<VpnProfile[]>("vpn").catch(() => null);
  vpn.profiles = Array.isArray(data) ? data : [];
  // Global connections left running by a previous session (crash / keep-on-exit).
  const adopted = await invoke<VpnStatus[]>("vpn_adopt").catch(() => []);
  if (adopted.length) toast(`Önceki oturumdan açık VPN: ${adopted.map((a) => a.name).join(", ")}`);
  for (const s of await invoke<VpnStatus[]>("vpn_list").catch(() => [])) vpn.status[s.id] = s;
  await refreshComponent();
}

export async function refreshComponent() {
  const st = await invoke<{ source: "downloaded" | "system" | "missing" }>("vpn_component_status", { component: "openconnect", tool: "openconnect" }).catch(() => null);
  vpn.component = st?.source ?? "missing";
}

export async function installComponent(): Promise<boolean> {
  vpn.download = { component: "openconnect", done: 0, total: 0 };
  try {
    await invoke("vpn_component_install", { component: "openconnect" });
    await refreshComponent();
    toast("OpenConnect indirildi");
    return true;
  } catch (e) {
    const m = String(e);
    toast(m.includes("NOT_AVAILABLE") ? "Bu platform için OpenConnect paketi henüz yayınlanmadı" : `İndirilemedi: ${m}`);
    return false;
  } finally {
    vpn.download = null;
  }
}

export function newProfile(): VpnProfile {
  return { id: uid(), name: "", protocol: "openconnect", ocProtocol: "gp", server: "", user: "", defaultMode: appModeSupported() ? "app" : "global" };
}

export async function saveProfile(p: VpnProfile, password?: string | null) {
  const i = vpn.profiles.findIndex((x) => x.id === p.id);
  if (i >= 0) vpn.profiles[i] = { ...p };
  else vpn.profiles.push({ ...p });
  if (password !== undefined) await invoke("vpn_secret_set", { id: p.id, secret: password });
  await persist();
}

export async function removeProfile(id: string) {
  await disconnect(id);
  vpn.profiles = vpn.profiles.filter((p) => p.id !== id);
  await invoke("vpn_secret_set", { id, secret: null }).catch(() => {});
  await persist();
}

const live = (s?: VpnStatus) => !!s && (s.state === "connecting" || s.state === "connected" || s.state === "reconnecting");
export const activeConnections = () => Object.values(vpn.status).filter(live);

/** Connect; resolves when connected (or rejects with the reason). */
export async function connect(p: VpnProfile, mode: VpnMode = p.defaultMode, password?: string): Promise<VpnStatus> {
  if (mode === "app" && !appModeSupported()) mode = "global";
  const cur = vpn.status[p.id];
  if (live(cur) && cur.state === "connected" && cur.mode === mode) return cur;
  if (live(cur)) await disconnect(p.id);
  if (vpn.component === "missing" || vpn.component === "unknown") {
    await refreshComponent();
    if (vpn.component === "missing") {
      const ok = await confirm({ title: "OpenConnect gerekli", message: "VPN bileşeni bu bilgisayarda yok. Şimdi indirilsin mi?", okText: "İndir" });
      if (!ok || !(await installComponent())) throw new Error("OpenConnect yok");
    }
  }
  const req = { ...p, mode, password, savePassword: false };
  return new Promise((resolve, reject) => {
    waiters.set(p.id, { resolve, reject, profile: p, mode });
    invoke<VpnStatus>("vpn_connect", { req }).then((s) => (vpn.status[p.id] = s)).catch((e) => {
      waiters.delete(p.id);
      reject(new Error(String(e)));
    });
  });
}

export async function disconnect(id: string) {
  reconnecting.delete(id);
  await invoke("vpn_disconnect", { id }).catch(() => {});
}

/** For SSH: SOCKS port of the server's linked VPN (connecting it first when needed). */
export async function proxyFor(vpnId?: string): Promise<string | undefined> {
  if (!vpnId) return undefined;
  const p = vpn.profiles.find((x) => x.id === vpnId);
  if (!p) return undefined;
  const s = await connect(p, vpn.status[p.id]?.state === "connected" ? vpn.status[p.id].mode : p.defaultMode);
  return s.mode === "app" && s.socksPort ? `127.0.0.1:${s.socksPort}` : undefined;
}

const waiters = new Map<string, { resolve: (s: VpnStatus) => void; reject: (e: Error) => void; profile: VpnProfile; mode: VpnMode }>();
const reconnecting = new Set<string>();

async function onStatus(s: VpnStatus) {
  const prev = vpn.status[s.id];
  vpn.status[s.id] = s;
  const w = waiters.get(s.id);
  if (s.state === "connected") {
    reconnecting.delete(s.id);
    if (w) { waiters.delete(s.id); w.resolve(s); }
    return;
  }
  if (s.state !== "error") {
    if (s.state === "disconnected" && w) { waiters.delete(s.id); w.reject(new Error("Bağlantı kesildi")); }
    return;
  }
  const p = vpn.profiles.find((x) => x.id === s.id);
  // Unknown server certificate: show the pin, remember it if the user trusts it, retry.
  if (s.errorKind === "cert" && s.pin && p && w) {
    waiters.delete(s.id);
    const ok = await confirm({
      title: "Sunucu sertifikası doğrulanamadı",
      message: `${p.server}\n${s.pin}\n\nBu sunucuya güveniyorsan sertifika bu profile kaydedilir.`,
      okText: "Güven ve bağlan",
    });
    if (!ok) return w.reject(new Error("Sertifika reddedildi"));
    p.servercert = s.pin;
    await saveProfile(p);
    connect(p, w.mode).then(w.resolve, w.reject);
    return;
  }
  // Dropped while connected: try once more on our own, then tell the user.
  if (s.errorKind === "network" && prev?.state === "connected" && p && !reconnecting.has(s.id)) {
    reconnecting.add(s.id);
    toast(`${p.name}: bağlantı koptu, yeniden bağlanılıyor…`);
    setTimeout(() => connect(p, s.mode).catch(() => toast(`${p.name}: yeniden bağlanılamadı`)), 2000);
    return;
  }
  if (w) { waiters.delete(s.id); w.reject(new Error(s.error ?? "VPN hatası")); }
  else if (prev && prev.state !== "error") toast(`${s.name}: ${s.error ?? "VPN hatası"}`);
}

async function onPrompt(e: { id: string; kind: string; message: string; options: string[] }) {
  const p = vpn.profiles.find((x) => x.id === e.id);
  const title = p?.name ?? "VPN";
  let answer: string | null = null;
  if (e.kind === "gateway" || e.kind === "group") {
    answer = await choose({ title: `${title} — ${e.kind === "gateway" ? "Gateway seç" : "Grup seç"}`, message: e.message, options: e.options });
  } else if (e.kind === "password") {
    const r = await prompt({ title, message: e.message, placeholder: "şifre", secret: true, remember: true });
    if (r && r.remember && p) invoke("vpn_secret_set", { id: p.id, secret: r.value }).catch(() => {});
    answer = r?.value ?? null;
  } else {
    const r = await prompt({ title: e.kind === "mfa" ? `${title} — doğrulama kodu` : title, message: e.message || undefined, placeholder: e.kind === "mfa" ? "kod / push için boş bırak" : "" });
    answer = r?.value ?? null;
  }
  if (answer === null) return disconnect(e.id);
  invoke("vpn_input", { id: e.id, text: answer }).catch(() => {});
}

export async function initVpnEvents() {
  await listen<VpnStatus>("vpn-status", (e) => onStatus(e.payload));
  await listen<{ id: string; kind: string; message: string; options: string[] }>("vpn-prompt", (e) => onPrompt(e.payload));
  await listen<{ component: string; done: number; total: number }>("vpn-download", (e) => (vpn.download = e.payload));
}

export const openVpnPage = () => (ui.page = "vpn");

export function fmtDuration(ms: number) {
  const s = Math.floor(ms / 1000), h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h ? `${h}s ${m}dk` : m ? `${m}dk ${s % 60}sn` : `${s}sn`;
}
