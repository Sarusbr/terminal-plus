// Theme definitions. Every color in the UI comes from these tokens.
import type { ITheme } from "@xterm/xterm";

export interface Theme {
  name: string;
  bg: string;
  fg: string;
  muted: string;
  accent: string;
  cursor: string;
  selection: string;
  ok: string;
  warn: string;
  danger: string;
  ansi: [string, string, string, string, string, string, string, string,
         string, string, string, string, string, string, string, string];
}

export const themes: Record<string, Theme> = {
  dracula: {
    name: "Dracula", bg: "#282a36", fg: "#f8f8f2", muted: "#6272a4", accent: "#bd93f9",
    cursor: "#f8f8f2", selection: "#44475a", ok: "#50fa7b", warn: "#f1fa8c", danger: "#ff5555",
    ansi: ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
           "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
  },
  nord: {
    name: "Nord", bg: "#2e3440", fg: "#d8dee9", muted: "#616e88", accent: "#88c0d0",
    cursor: "#d8dee9", selection: "#434c5e", ok: "#a3be8c", warn: "#ebcb8b", danger: "#bf616a",
    ansi: ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
           "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
  },
  onedark: {
    name: "One Dark", bg: "#1e2127", fg: "#abb2bf", muted: "#5c6370", accent: "#61afef",
    cursor: "#528bff", selection: "#3e4451", ok: "#98c379", warn: "#e5c07b", danger: "#e06c75",
    ansi: ["#1e2127", "#e06c75", "#98c379", "#d19a66", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
           "#5c6370", "#e06c75", "#98c379", "#d19a66", "#61afef", "#c678dd", "#56b6c2", "#ffffff"],
  },
  solarized: {
    name: "Solarized Dark", bg: "#002b36", fg: "#93a1a1", muted: "#586e75", accent: "#268bd2",
    cursor: "#93a1a1", selection: "#073642", ok: "#859900", warn: "#b58900", danger: "#dc322f",
    ansi: ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
           "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
  },
  graphite: {
    name: "Graphite", bg: "#16181d", fg: "#d7dae0", muted: "#6b717d", accent: "#7aa2f7",
    cursor: "#d7dae0", selection: "#2d323c", ok: "#8be9a8", warn: "#e5c07b", danger: "#f7768e",
    ansi: ["#1d2027", "#f7768e", "#8be9a8", "#e5c07b", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
           "#454a55", "#ff8fa3", "#a6f0bc", "#f0d197", "#94b4ff", "#cdb2ff", "#a3dcff", "#ffffff"],
  },
};

function rgb(hex: string) {
  const n = parseInt(hex.slice(1), 16);
  return `${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255}`;
}

/** Push theme tokens to CSS custom properties. */
export function applyThemeCss(t: Theme, opacity: number) {
  const s = document.documentElement.style;
  s.setProperty("--bg-rgb", rgb(t.bg));
  s.setProperty("--fg-rgb", rgb(t.fg));
  s.setProperty("--bg-opacity", String(opacity));
  s.setProperty("--fg", t.fg);
  s.setProperty("--cursor", t.cursor);
  s.setProperty("--muted", t.muted);
  s.setProperty("--accent", t.accent);
  s.setProperty("--ok", t.ok);
  s.setProperty("--warn", t.warn);
  s.setProperty("--danger", t.danger);
  s.setProperty("--selection", t.selection);
  t.ansi.forEach((c, i) => s.setProperty(`--ansi-${i}`, c));
}

export function xtermTheme(t: Theme): ITheme {
  const [black, red, green, yellow, blue, magenta, cyan, white,
    brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite] = t.ansi;
  return {
    background: "rgba(0,0,0,0)", foreground: t.fg, cursor: t.cursor, cursorAccent: t.bg,
    selectionBackground: t.selection + "cc",
    black, red, green, yellow, blue, magenta, cyan, white,
    brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite,
  };
}
