// One round trip collects everything for the System page. Linux uses /proc; macOS
// (local tabs) uses sysctl/vm_stat/top. Output is split by @@SECTION markers.
import type { Shell } from "../../lib/shell";

const LINUX = [
  "echo @@HOST; hostname",
  "echo @@UNAME; uname -srmo 2>/dev/null || uname -a",
  "echo @@OS; . /etc/os-release 2>/dev/null && echo \"$PRETTY_NAME\"",
  "echo @@UP; cat /proc/uptime",
  "echo @@LOAD; cat /proc/loadavg",
  "echo @@NPROC; nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo",
  "echo @@MEM; grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo",
  "echo @@DF; df -kP -x tmpfs -x devtmpfs -x overlay -x squashfs -x efivarfs 2>/dev/null || df -kP",
  "echo @@CPU; head -1 /proc/stat; sleep 0.5; head -1 /proc/stat",
];
const DARWIN = [
  "echo @@HOST; hostname",
  "echo @@UNAME; uname -srm",
  "echo @@OS; echo \"macOS $(sw_vers -productVersion)\"",
  "echo @@UPBOOT; sysctl -n kern.boottime",
  "echo @@LOAD; sysctl -n vm.loadavg",
  "echo @@NPROC; sysctl -n hw.ncpu",
  "echo @@MEMMAC; sysctl -n hw.memsize hw.pagesize; vm_stat",
  "echo @@SWAPMAC; sysctl -n vm.swapusage",
  "echo @@DF; df -kP",
  "echo @@CPUMAC; top -l 2 -n 0 -s 1 | grep 'CPU usage' | tail -1",
];

export const SCRIPT = `case "$(uname -s)" in Darwin) ${DARWIN.join("; ")};; *) ${LINUX.join("; ")};; esac`;

export interface Disk { fs: string; mount: string; size: number; used: number; pct: number }
export interface SysInfo {
  host: string; uname: string; os: string; uptime: number; load: number[]; cores: number;
  memTotal: number; memUsed: number; swapTotal: number; swapUsed: number; cpu: number | null; disks: Disk[];
}

function sections(out: string) {
  const map: Record<string, string[]> = {};
  let cur = "";
  for (const line of out.split("\n")) {
    const m = /^@@(\w+)$/.exec(line.trim());
    if (m) { cur = m[1]; map[cur] = []; } else if (cur) map[cur].push(line);
  }
  return map;
}

export async function collect(shell: Shell): Promise<SysInfo> {
  const s = sections(await shell.ok(SCRIPT, 15000));
  const info: SysInfo = {
    host: s.HOST?.[0]?.trim() ?? "", uname: s.UNAME?.[0]?.trim() ?? "", os: s.OS?.[0]?.trim() ?? "",
    uptime: 0, load: [], cores: Number(s.NPROC?.[0]) || 1, memTotal: 0, memUsed: 0, swapTotal: 0, swapUsed: 0, cpu: null, disks: [],
  };
  if (s.UP) info.uptime = parseFloat(s.UP[0]) || 0;
  if (s.UPBOOT) {
    const m = /sec = (\d+)/.exec(s.UPBOOT.join(" "));
    if (m) info.uptime = Date.now() / 1000 - Number(m[1]);
  }
  if (s.LOAD) info.load = s.LOAD.join(" ").replace(/[{}]/g, "").trim().split(/\s+/).slice(0, 3).map(Number);
  if (s.MEM) {
    const kv: Record<string, number> = {};
    for (const l of s.MEM) { const m = /^(\w+):\s+(\d+)/.exec(l); if (m) kv[m[1]] = Number(m[2]) * 1024; }
    info.memTotal = kv.MemTotal ?? 0;
    info.memUsed = info.memTotal - (kv.MemAvailable ?? 0);
    info.swapTotal = kv.SwapTotal ?? 0;
    info.swapUsed = info.swapTotal - (kv.SwapFree ?? 0);
  }
  if (s.MEMMAC) {
    const total = Number(s.MEMMAC[0]) || 0;
    const page = Number(s.MEMMAC[1]) || 4096;
    const pages = (name: string) => {
      const l = s.MEMMAC.find((x) => x.startsWith(name));
      return l ? Number(l.replace(/\D+/g, " ").trim().split(" ").pop()) : 0;
    };
    // Same idea as Activity Monitor's "Memory Used": app (anonymous) + wired + compressed.
    const used = (pages("Anonymous pages") - pages("Pages purgeable") + pages("Pages wired down") + pages("Pages occupied by compressor")) * page;
    info.memTotal = total;
    info.memUsed = Math.max(0, Math.min(total, used || (total - (pages("Pages free") + pages("Pages inactive")) * page)));
  }
  if (s.SWAPMAC) {
    const m = /total = ([\d.]+)M\s+used = ([\d.]+)M/.exec(s.SWAPMAC.join(" "));
    if (m) { info.swapTotal = Number(m[1]) * 1048576; info.swapUsed = Number(m[2]) * 1048576; }
  }
  if (s.CPU?.length >= 2) {
    const parse = (l: string) => l.trim().split(/\s+/).slice(1).map(Number);
    const a = parse(s.CPU[0]), b = parse(s.CPU[1]);
    const idle = (x: number[]) => x[3] + (x[4] || 0);
    const sum = (x: number[]) => x.reduce((p, c) => p + c, 0);
    const dt = sum(b) - sum(a);
    if (dt > 0) info.cpu = Math.max(0, Math.min(100, (1 - (idle(b) - idle(a)) / dt) * 100));
  }
  if (s.CPUMAC) {
    const m = /([\d.]+)% idle/.exec(s.CPUMAC.join(" "));
    if (m) info.cpu = 100 - Number(m[1]);
  }
  for (const l of s.DF ?? []) {
    const p = l.trim().split(/\s+/);
    if (p.length < 6 || p[0] === "Filesystem" || !/^\d+$/.test(p[1])) continue;
    const mount = p.slice(5).join(" ");
    if (/^\/(System\/Volumes\/(VM|Preboot|Update|xarts|iSCPreboot|Hardware)|dev|private\/var\/vm)/.test(mount)) continue;
    const size = Number(p[1]) * 1024;
    if (!size) continue;
    info.disks.push({ fs: p[0], mount, size, used: Number(p[2]) * 1024, pct: parseInt(p[4]) || 0 });
  }
  return info;
}

// ── Windows (local tabs): one PowerShell/CIM call, JSON out ─────────────────
const WIN_SCRIPT = `
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$os = Get-CimInstance Win32_OperatingSystem
$p = @(Get-CimInstance Win32_Processor)
$pf = @(Get-CimInstance Win32_PageFileUsage)
$disks = @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object {
  @{ mount = $_.DeviceID; fs = (($_.VolumeName + ' ' + $_.FileSystem).Trim()); size = [double]$_.Size; free = [double]$_.FreeSpace } })
@{
  host = $env:COMPUTERNAME; os = $os.Caption; ver = $os.Version; arch = $os.OSArchitecture
  uptime = [double]((Get-Date) - $os.LastBootUpTime).TotalSeconds
  memTotal = [double]$os.TotalVisibleMemorySize * 1024; memFree = [double]$os.FreePhysicalMemory * 1024
  cpu = ($p | Measure-Object -Property LoadPercentage -Average).Average
  cores = ($p | Measure-Object -Property NumberOfLogicalProcessors -Sum).Sum
  swapTotal = [double](($pf | Measure-Object -Property AllocatedBaseSize -Sum).Sum) * 1MB
  swapUsed = [double](($pf | Measure-Object -Property CurrentUsage -Sum).Sum) * 1MB
  disks = $disks
} | ConvertTo-Json -Compress -Depth 4`;

interface WinRaw {
  host: string; os: string; ver: string; arch: string; uptime: number; memTotal: number; memFree: number;
  cpu: number | null; cores: number; swapTotal: number; swapUsed: number;
  disks: { mount: string; fs: string; size: number; free: number }[];
}

export function parseWindows(out: string): SysInfo {
  const j = JSON.parse(out.trim()) as WinRaw;
  const disks = (Array.isArray(j.disks) ? j.disks : j.disks ? [j.disks] : []).filter((d) => d.size > 0);
  return {
    host: j.host, os: j.os, uname: `Windows ${j.ver} ${j.arch ?? ""}`.trim(), uptime: j.uptime || 0, load: [],
    cores: j.cores || 1, memTotal: j.memTotal || 0, memUsed: Math.max(0, (j.memTotal || 0) - (j.memFree || 0)),
    swapTotal: j.swapTotal || 0, swapUsed: j.swapUsed || 0, cpu: j.cpu ?? null,
    disks: disks.map((d) => ({
      fs: d.fs || d.mount, mount: d.mount, size: d.size, used: d.size - d.free,
      pct: Math.round(((d.size - d.free) / d.size) * 100),
    })),
  };
}

export async function collectWindows(): Promise<SysInfo> {
  const { winExec } = await import("../../lib/winlocal");
  // -EncodedCommand (UTF-16LE base64) avoids every command-line quoting/newline pitfall.
  const bytes = new Uint8Array(WIN_SCRIPT.length * 2);
  for (let i = 0; i < WIN_SCRIPT.length; i++) {
    const c = WIN_SCRIPT.charCodeAt(i);
    bytes[i * 2] = c & 0xff;
    bytes[i * 2 + 1] = c >> 8;
  }
  const enc = btoa(String.fromCharCode(...bytes));
  return parseWindows(await winExec("powershell.exe", ["-NoProfile", "-NonInteractive", "-EncodedCommand", enc], undefined, 30000));
}
