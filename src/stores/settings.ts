import { reactive, watch } from "vue";
import { api } from "../lib/api";
import { applyThemeCss, themes, type Theme } from "../lib/themes";
import { toast } from "./ui";

export interface Settings {
  theme: string;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  opacity: number;
  blur: boolean;
  cursorStyle: "block" | "bar" | "underline";
  cursorBlink: boolean;
  scrollback: number;
  bell: "none" | "visual";
  blurLevel: "light" | "medium" | "strong";
  ligatures: boolean;
  /** Local shell id from `local_shells` ("" = first: CMD on Windows, $SHELL elsewhere). */
  localShell: string;
  /** Leave a global VPN up when the app quits (read by the Rust side on exit). */
  vpnKeepGlobal: boolean;
  /** Per-token overrides on top of the selected theme. */
  custom: { bg?: string; fg?: string; accent?: string; cursor?: string; selection?: string };
}

const defaults: Settings = {
  theme: "graphite",
  fontFamily: 'ui-monospace, "SF Mono", Menlo, "Cascadia Code", Consolas, "DejaVu Sans Mono", monospace',
  fontSize: 13,
  lineHeight: 1.2,
  opacity: 0.86,
  blur: true,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 10000,
  bell: "visual",
  blurLevel: "medium",
  ligatures: false,
  localShell: "",
  vpnKeepGlobal: false,
  custom: {},
};

export const settings = reactive<Settings>({ ...defaults });
export const platform = { os: "macos", mod: "Meta" as "Meta" | "Control" };

export const currentTheme = (): Theme => {
  const base = themes[settings.theme] ?? themes.graphite;
  const c = Object.fromEntries(Object.entries(settings.custom ?? {}).filter(([, v]) => !!v));
  return { ...base, ...c };
};

let saveTimer: number | undefined;

export async function loadSettings() {
  platform.os = await api.platform().catch(() => "macos");
  platform.mod = platform.os === "macos" ? "Meta" : "Control";
  document.documentElement.dataset.os = platform.os;
  const saved = await api.storeLoad<Partial<Settings>>("settings").catch(() => null);
  if (saved) Object.assign(settings, { ...saved, custom: { ...(saved.custom ?? {}) } });
  apply();
  applyEffect();

  watch(settings, () => {
    apply();
    clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => api.storeSave("settings", { ...settings }), 300);
  }, { deep: true });
  watch(() => [settings.blur, settings.blurLevel] as const, applyEffect);
}

/** Native blur behind the window; tell the user when this OS can't do it. */
function applyEffect() {
  api.setWindowEffect(settings.blur, settings.blurLevel).catch((e) => {
    if (settings.blur) toast(`Pencere efekti uygulanamadı (${String(e)}). Opaklık ayarı yine çalışır.`);
  });
}

function apply() {
  applyThemeCss(currentTheme(), settings.opacity);
  const s = document.documentElement.style;
  s.setProperty("--font-mono", settings.fontFamily);
  s.setProperty("--font-size", settings.fontSize + "px");
}

export function resetSettings() {
  Object.assign(settings, { ...defaults, custom: {} });
}
