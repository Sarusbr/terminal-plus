// Docker CLI access. POSIX shells: typed through the shell runner (sudo fallback when the
// socket isn't accessible, remembered per tab). Windows local: docker.exe run directly.
import { api } from "../../lib/api";
import { sh, q } from "../../lib/shell";
import { isWinLocal, cwdOf } from "../../lib/winlocal";
import { paneHandles, type Tab } from "../../stores/tabs";

const useSudo = new WeakMap<Tab, boolean>();

export class DockerError extends Error {
  constructor(public kind: "missing" | "daemon" | "other", msg: string) { super(msg); }
}

/** Quote for cmd.exe / PowerShell command lines (visible commands only). */
const winQuote = (a: string) => (/[\s"&|<>^;]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a);

export function docker(tab: Tab) {
  const win = isWinLocal(tab);
  const prefix = () => (useSudo.get(tab) ? "sudo -p '[sudo] password for %u: ' docker" : "docker");
  // Quote only when needed so commands typed into the terminal stay readable.
  const arg = (a: string) => (/^[\w@%+=:,./-]+$/.test(a) ? a : q(a));
  const line = (args: string[]) => `${prefix()} ${args.map(arg).join(" ")}`;

  async function exec(args: string[], timeout: number): Promise<{ code: number; out: string }> {
    if (win) {
      try {
        const r = await api.localExec("docker", args, cwdOf(tab), timeout);
        return { code: r.code, out: r.out + (r.err ? "\n" + r.err : "") };
      } catch (e) {
        if (String(e).includes("NOT_FOUND")) return { code: 127, out: "docker: command not found" };
        throw e;
      }
    }
    return sh(tab).run(line(args), timeout);
  }

  async function run(args: string[], timeout = 30000): Promise<string> {
    let r = await exec(args, timeout);
    if (r.code !== 0 && !win && !useSudo.get(tab) && /permission denied.*docker\.sock|docker\.sock.*permission denied/is.test(r.out)) {
      useSudo.set(tab, true);
      r = await exec(args, timeout);
    }
    if (r.code === 127 || (/command not found|not found: docker|is not recognized/i.test(r.out) && r.code !== 0))
      throw new DockerError("missing", "Docker bulunamadı");
    if (r.code !== 0) {
      const msg = r.out.trim().split(/\r?\n/).slice(-2).join("\n") || `exit ${r.code}`;
      throw new DockerError(/Cannot connect to the Docker daemon|daemon running|docker_engine|pipe/i.test(msg) ? "daemon" : "other", msg);
    }
    return r.out;
  }

  /** Type a docker command into the terminal, visibly (exec -it, logs -f). */
  function type(args: string[]) {
    if (win) paneHandles.get(tab.activePane)?.write(["docker", ...args.map(winQuote)].join(" ") + "\r");
    else sh(tab).type(line(args));
  }

  const jsonLines = <T>(out: string): T[] =>
    out.split(/\r?\n/).map((l) => l.trim()).filter((l) => l.startsWith("{")).map((l) => {
      try { return JSON.parse(l) as T; } catch { return null; }
    }).filter(Boolean) as T[];

  return {
    run,
    type,
    jsonLines,
    exec(id: string) {
      type(["exec", "-it", id, "sh", "-c", "command -v bash >/dev/null && exec bash || exec sh"]);
    },
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
