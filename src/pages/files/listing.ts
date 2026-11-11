// Directory listing: local via Rust fs, remote via a machine-readable `find -printf`
// (GNU) with an `ls -la` fallback (busybox/BSD).
import { api, type FsEntry } from "../../lib/api";
import type { Shell } from "../../lib/shell";

const FIND =
  `LC_ALL=C find . -mindepth 1 -maxdepth 1 -printf '%y\\t%m\\t%s\\t%T@\\t%Y\\t%f\\t%l\\n' 2>/dev/null` +
  ` || { echo __TP_LS__; LC_ALL=C ls -la; }`;

function parseFind(lines: string[]): FsEntry[] {
  const out: FsEntry[] = [];
  for (const l of lines) {
    const p = l.split("\t");
    if (p.length < 6) continue;
    const kind = p[0] === "d" ? "d" : p[0] === "f" ? "f" : p[0] === "l" ? "l" : "o";
    out.push({
      name: p[5], kind, mode: parseInt(p[1], 8), size: Number(p[2]) || 0,
      mtime: Math.floor(Number(p[3]) || 0), linkDir: p[4] === "d", link: p[6] || null,
    });
  }
  return out;
}

const PERM = { r: 4, w: 2, x: 1, s: 1, t: 1, S: 0, T: 0, "-": 0 } as Record<string, number>;
function permBits(s: string) {
  let m = 0;
  for (let i = 0; i < 9; i++) m = (m << 1) | (PERM[s[i]] ? 1 : 0);
  return m;
}

function parseLs(lines: string[]): FsEntry[] {
  const out: FsEntry[] = [];
  const re = /^([-dlcbps])([rwxsStT-]{9})\S*\s+\d+\s+\S+\s+\S+\s+(\d+)\s+(\S+\s+\S+\s+\S+)\s+(.+)$/;
  for (const l of lines) {
    const m = re.exec(l);
    if (!m) continue;
    let name = m[5];
    let link: string | null = null;
    if (m[1] === "l" && name.includes(" -> ")) [name, link] = name.split(" -> ");
    if (name === "." || name === "..") continue;
    const t = Date.parse(m[4]);
    out.push({
      name, kind: m[1] === "d" ? "d" : m[1] === "-" ? "f" : m[1] === "l" ? "l" : "o",
      mode: permBits(m[2]), size: Number(m[3]), mtime: isNaN(t) ? 0 : t / 1000, link, linkDir: false,
    });
  }
  return out;
}

export async function listCwd(shell: Shell): Promise<{ cwd: string; entries: FsEntry[] }> {
  if (shell.tab.kind === "local") {
    const cwd = (await shell.ok("pwd")).trim();
    return { cwd, entries: await api.localListDir(cwd) };
  }
  const out = await shell.ok(`pwd; ${FIND}`, 30000);
  const lines = out.split("\n");
  const cwd = (lines.shift() ?? "/").trim();
  const ls = lines.indexOf("__TP_LS__");
  return { cwd, entries: ls >= 0 ? parseLs(lines.slice(ls + 1)) : parseFind(lines) };
}

export function fmtSize(n: number) {
  if (n < 1024) return `${n} B`;
  const u = ["K", "M", "G", "T"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}

export function fmtMode(kind: string, mode: number) {
  const c = kind === "d" ? "d" : kind === "l" ? "l" : "-";
  let s = "";
  for (let i = 8; i >= 0; i--) s += mode & (1 << i) ? "rwx"[(8 - i) % 3] : "-";
  return c + s;
}

export function fmtDate(sec: number) {
  if (!sec) return "";
  const d = new Date(sec * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
