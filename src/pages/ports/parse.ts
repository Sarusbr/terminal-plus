export interface Port { proto: string; addr: string; port: number; pid?: number; proc?: string }

// ss (Linux) → netstat (older Linux/busybox) → lsof (macOS).
export const SCRIPT = (s: string) =>
  `if command -v ss >/dev/null 2>&1; then echo @@SS; ${s}ss -tulpnH; ` +
  `elif command -v netstat >/dev/null 2>&1 && netstat -tulpn >/dev/null 2>&1; then echo @@NS; ${s}netstat -tulpn; ` +
  `else echo @@LSOF; ${s}lsof -nP -iTCP -sTCP:LISTEN -FpcnPt; ${s}lsof -nP -iUDP -FpcnPt; fi`;

function splitAddr(a: string) {
  const i = a.lastIndexOf(":");
  return { addr: a.slice(0, i).replace(/^\[|\]$/g, "") || "*", port: Number(a.slice(i + 1)) || 0 };
}

export function parse(out: string): { ports: Port[]; source: string } {
  let source = "";
  const lines = out.split("\n");
  const kind = lines.shift()?.trim();
  const res: Port[] = [];
  if (kind === "@@SS") {
    source = "ss";
    for (const l of lines) {
      const p = l.trim().split(/\s+/);
      if (p.length < 5) continue;
      const m = /\("([^"]+)",pid=(\d+)/.exec(l);
      res.push({ proto: p[0], ...splitAddr(p[4]), proc: m?.[1], pid: m ? Number(m[2]) : undefined });
    }
  } else if (kind === "@@NS") {
    source = "netstat";
    for (const l of lines) {
      const p = l.trim().split(/\s+/);
      if (!/^(tcp|udp)/.test(p[0] ?? "")) continue;
      const last = p[p.length - 1];
      const m = /^(\d+)\/(.+)$/.exec(last);
      res.push({ proto: p[0].replace(/6$/, ""), ...splitAddr(p[3]), pid: m ? Number(m[1]) : undefined, proc: m?.[2] });
    }
  } else {
    source = "lsof";
    let pid = 0, cmd = "", proto = "";
    for (const l of lines) {
      const t = l[0], v = l.slice(1);
      if (t === "p") pid = Number(v);
      else if (t === "c") cmd = v;
      else if (t === "P") proto = v.toLowerCase();
      else if (t === "n" && !v.includes("->")) {
        const a = splitAddr(v);
        if (a.port) res.push({ proto, ...a, pid, proc: cmd });
      }
    }
  }
  const seen = new Set<string>();
  const res2 = res.filter((p) => {
    const k = `${p.proto}|${p.addr}|${p.port}|${p.pid}`;
    return seen.has(k) ? false : (seen.add(k), true);
  }).sort((a, b) => a.port - b.port || a.proto.localeCompare(b.proto));
  return { ports: res2, source };
}

