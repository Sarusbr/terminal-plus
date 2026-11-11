// Docker CLI access through the shell runner. Falls back to sudo when the socket is
// not accessible to the user (the choice is remembered per tab).
import { sh, q, type Shell } from "../../lib/shell";
import type { Tab } from "../../stores/tabs";

const useSudo = new WeakMap<Tab, boolean>();

export class DockerError extends Error {
  constructor(public kind: "missing" | "daemon" | "other", msg: string) { super(msg); }
}

export function docker(tab: Tab) {
  const shell: Shell = sh(tab);
  const prefix = () => (useSudo.get(tab) ? "sudo -p '[sudo] password for %u: ' docker" : "docker");

  async function run(args: string, timeout = 30000): Promise<string> {
    let r = await shell.run(`${prefix()} ${args}`, timeout);
    if (r.code !== 0 && !useSudo.get(tab) && /permission denied.*docker\.sock|docker\.sock.*permission denied/is.test(r.out)) {
      useSudo.set(tab, true);
      r = await shell.run(`${prefix()} ${args}`, timeout);
    }
    if (r.code === 127 || /command not found|not found: docker/i.test(r.out) && r.code !== 0)
      throw new DockerError("missing", "Docker bulunamadı");
    if (r.code !== 0) {
      const msg = r.out.trim().split("\n").slice(-2).join("\n") || `exit ${r.code}`;
      throw new DockerError(/Cannot connect to the Docker daemon|daemon running/i.test(msg) ? "daemon" : "other", msg);
    }
    return r.out;
  }

  const jsonLines = <T>(out: string): T[] =>
    out.split("\n").map((l) => l.trim()).filter((l) => l.startsWith("{")).map((l) => {
      try { return JSON.parse(l) as T; } catch { return null; }
    }).filter(Boolean) as T[];

  return {
    run,
    jsonLines,
    exec(id: string) {
      shell.type(`${prefix()} exec -it ${q(id)} sh -c 'command -v bash >/dev/null && exec bash || exec sh'`);
    },
    q,
  };
}

export interface Container {
  ID: string; Names: string; Image: string; State: string; Status: string; Ports: string;
  RunningFor: string; Labels?: string;
}
export interface Image { ID: string; Repository: string; Tag: string; Size: string; CreatedSince: string }
export interface Volume { Name: string; Driver: string; Mountpoint?: string }
export interface Network { ID: string; Name: string; Driver: string; Scope: string }
export interface Compose { Name: string; Status: string; ConfigFiles: string }
export interface Stats { Container: string; Name: string; CPUPerc: string; MemUsage: string; MemPerc: string; NetIO: string; BlockIO: string; PIDs: string }
