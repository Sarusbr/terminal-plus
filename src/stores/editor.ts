// Built-in editor state: open, load (with sudo fallback), save (with sudo fallback).
import { reactive } from "vue";
import { api } from "../lib/api";
import { sh, q, isPermissionError } from "../lib/shell";
import type { Tab } from "./tabs";
import { ui, toast } from "./ui";

export interface EditorDoc {
  tab: Tab;
  path: string;
  content: string;
  /** Content as last loaded/saved — dirty = content !== saved. */
  saved: string;
  exists: boolean;
  sudo: boolean;
  loading: boolean;
  saving: boolean;
  error: string;
  /** Page to return to on close. */
  back: string;
}

export const editor = reactive<{ doc: EditorDoc | null }>({ doc: null });

const target = (tab: Tab) => (tab.kind === "local" ? null : tab.connId);

/** Resolve relative / ~ paths against the shell's cwd and $HOME. */
async function resolvePath(tab: Tab, raw: string): Promise<string> {
  if (raw.startsWith("/")) return raw;
  const out = await sh(tab).ok(`pwd; printf '%s\\n' "$HOME"`);
  const [cwd, home] = out.split("\n").map((s) => s.trim());
  if (raw === "~") return home;
  if (raw.startsWith("~/")) return home + raw.slice(1);
  return (cwd === "/" ? "" : cwd) + "/" + raw.replace(/^\.\//, "");
}

function b64ToText(b64: string): string {
  const bin = atob(b64.replace(/\s+/g, ""));
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

export async function openEditor(tab: Tab, rawPath: string, opts: { sudo?: boolean } = {}) {
  const doc: EditorDoc = reactive({
    tab, path: rawPath, content: "", saved: "", exists: true, sudo: !!opts.sudo,
    loading: true, saving: false, error: "", back: ui.page === "editor" ? "terminal" : ui.page,
  });
  editor.doc = doc;
  ui.page = "editor";
  try {
    doc.path = await resolvePath(tab, rawPath);
    let text: string;
    try {
      if (doc.sudo) throw new Error("PERMISSION_DENIED");
      const r = await api.fileRead(target(tab), doc.path);
      text = r.content;
      doc.exists = r.exists;
    } catch (e) {
      const msg = String((e as Error)?.message ?? e);
      if (!msg.includes("PERMISSION_DENIED")) throw e;
      doc.sudo = true;
      const out = await sh(tab).ok(`sudo -p '[sudo] password for %u: ' cat -- ${q(doc.path)} | base64`, 60000);
      text = b64ToText(out);
    }
    doc.content = doc.saved = text;
  } catch (e) {
    const m = String((e as Error)?.message ?? e);
    doc.error = m === "BINARY" ? "İkili dosya düzenlenemez" : m === "TOO_LARGE" ? "Dosya çok büyük" : m;
  } finally {
    doc.loading = false;
  }
}

export async function saveEditor(): Promise<boolean> {
  const doc = editor.doc;
  if (!doc || doc.loading || doc.saving || doc.error) return false;
  doc.saving = true;
  const content = doc.content;
  try {
    let viaSudo = doc.sudo;
    if (!viaSudo) {
      try {
        await api.fileWrite(target(doc.tab), doc.path, content);
      } catch (e) {
        if (!String(e).includes("PERMISSION_DENIED")) throw e;
        viaSudo = true;
      }
    }
    if (viaSudo) {
      const tmp = await api.tempWrite(target(doc.tab), content);
      const r = await sh(doc.tab).run(
        `sudo -p '[sudo] password for %u: ' cp -- ${q(tmp)} ${q(doc.path)}; tp_r=$?; rm -f -- ${q(tmp)}; [ $tp_r -eq 0 ]`,
        60000,
      );
      if (r.code !== 0) throw new Error(isPermissionError(r.out) ? "Yetki yok" : r.out.trim() || "sudo başarısız");
      doc.sudo = true;
    }
    doc.saved = content;
    doc.exists = true;
    return true;
  } catch (e) {
    toast(`Kaydedilemedi: ${String((e as Error)?.message ?? e)}`);
    return false;
  } finally {
    doc.saving = false;
  }
}

export function closeEditor() {
  const doc = editor.doc;
  editor.doc = null;
  ui.page = doc?.back && doc.back !== "editor" ? doc.back : "terminal";
}
