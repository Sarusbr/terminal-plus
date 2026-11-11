import { reactive } from "vue";
import { api, uid } from "../lib/api";

export interface Server {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  auth: "password" | "key";
  keyPath?: string;
  group?: string;
  tags: string[];
  /** Optional VPN profile opened (in its default mode) before connecting. */
  vpnId?: string;
}

export const servers = reactive<{ list: Server[]; loaded: boolean }>({ list: [], loaded: false });

export async function loadServers() {
  const data = await api.storeLoad<Server[]>("servers").catch(() => null);
  servers.list = Array.isArray(data) ? data.map((s) => ({ ...s, tags: s.tags ?? [] })) : [];
  servers.loaded = true;
}

const persist = () => api.storeSave("servers", servers.list);

export function newServer(partial: Partial<Server> = {}): Server {
  return { id: uid(), name: "", host: "", port: 22, user: "root", auth: "key", tags: [], ...partial };
}

export async function upsertServer(s: Server) {
  const i = servers.list.findIndex((x) => x.id === s.id);
  if (i >= 0) servers.list[i] = { ...s };
  else servers.list.push({ ...s });
  await persist();
}

export async function removeServer(id: string) {
  servers.list = servers.list.filter((s) => s.id !== id);
  await Promise.all([api.secretDelete("pw", id), api.secretDelete("pp", id)]).catch(() => {});
  await persist();
}

/** Import hosts from ~/.ssh/config; skips ones already saved (same host+port+user). */
export async function importSshConfig(): Promise<number> {
  const hosts = await api.sshConfigImport();
  let added = 0;
  for (const h of hosts) {
    const user = h.user ?? "root";
    if (servers.list.some((s) => s.host === h.host && s.port === h.port && s.user === user)) continue;
    servers.list.push(newServer({
      name: h.name, host: h.host, port: h.port, user,
      auth: "key", keyPath: h.keyPath, group: "ssh config",
    }));
    added++;
  }
  if (added) await persist();
  return added;
}
