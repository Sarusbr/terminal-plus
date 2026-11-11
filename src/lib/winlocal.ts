// Windows local tabs (CMD / PowerShell). ConPTY keeps its own screen model, so panel
// commands are not typed into the shell here: they run as separate processes in the
// shell's directory (reported by the prompt via OSC 9;9). Only `cd` is typed, visibly.
import { api } from "./api";
import { platform } from "../stores/settings";
import { paneHandles, type Tab } from "../stores/tabs";

export const isWinLocal = (tab?: Tab) => !!tab && tab.kind === "local" && platform.os === "windows";

export const cwdOf = (tab: Tab) => tab.panes[tab.activePane]?.cwd;

export function winJoin(dir: string, name: string) {
  return dir.endsWith("\\") || dir.endsWith("/") ? dir + name : `${dir}\\${name}`;
}

export function winParent(dir: string) {
  const d = dir.replace(/[\\/]+$/, "");
  const i = d.lastIndexOf("\\");
  if (i < 0) return dir;
  return i <= 2 ? d.slice(0, i + 1) : d.slice(0, i); // "C:\x" → "C:\"
}

/** "C:\Users\me" → [C:\, C:\Users, C:\Users\me] */
export function winCrumbs(dir: string) {
  const parts = dir.replace(/[\\/]+$/, "").split("\\").filter(Boolean);
  if (!parts.length) return [];
  const root = parts[0] + "\\";
  return [{ name: root, path: root }, ...parts.slice(1).map((p, i) => ({ name: p, path: root + parts.slice(1, i + 2).join("\\") }))];
}

/** Type a `cd` into the terminal and wait for the prompt to report the new directory. */
export async function winCd(tab: Tab, path: string | "~"): Promise<string | undefined> {
  const h = paneHandles.get(tab.activePane);
  if (!h) throw new Error("Terminal bağlı değil");
  const before = cwdOf(tab);
  let cmd: string;
  if (tab.shellKind === "cmd") {
    cmd = path === "~" ? `cd /d "%USERPROFILE%"` : `cd /d "${path.replace(/"/g, "")}"`;
  } else {
    // Leading space: kept out of PSReadLine history by our prompt integration.
    cmd = path === "~" ? " Set-Location ~" : ` Set-Location -LiteralPath '${path.replace(/'/g, "''")}'`;
  }
  h.write(cmd + "\r");
  for (let i = 0; i < 40; i++) {
    await new Promise((r) => setTimeout(r, 75));
    const now = cwdOf(tab);
    if (now && now !== before) return now;
  }
  return cwdOf(tab);
}

export async function winHome(): Promise<string> {
  const r = await api.localExec("cmd.exe", ["/d", "/c", "echo %USERPROFILE%"]).catch(() => null);
  return r?.out.trim() || "C:\\";
}

/** Run a program; throws with its error text on non-zero exit. */
export async function winExec(program: string, args: string[], cwd?: string, timeoutMs?: number): Promise<string> {
  const r = await api.localExec(program, args, cwd, timeoutMs);
  if (r.code !== 0) throw new Error((r.err || r.out).trim().split(/\r?\n/).slice(-2).join("\n") || `exit ${r.code}`);
  return r.out;
}
