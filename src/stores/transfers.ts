// File transfers: start, track progress events, cancel.
import { reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { uid } from "../lib/api";
import type { Tab } from "./tabs";
import { toast } from "./ui";

export interface TFile { name: string; size: number }
export interface Transfer {
  id: string;
  up: boolean;
  label: string;
  files: TFile[];
  total: number;
  bytes: number;
  index: number;
  fileDone: number;
  started: number;
  speed: number;
  state: "planning" | "running" | "done" | "error" | "cancelled";
  error?: string;
  onDone?: () => void;
}

export const transfers = reactive<{ list: Transfer[]; version: number }>({ list: [], version: 0 });

/** Shared with the Files page so drops land in the directory being viewed. */
export const filesCwd = reactive<Record<string, string>>({});

const byId = (id: string) => transfers.list.find((t) => t.id === id);
let lastSample = new Map<string, { t: number; b: number }>();

export async function startTransfer(tab: Tab, direction: "up" | "down", sources: string[], destDir: string, onDone?: () => void) {
  const t: Transfer = reactive({
    id: uid(), up: direction === "up", label: tab.kind === "local" ? "local" : tab.server.host,
    files: [], total: 0, bytes: 0, index: 0, fileDone: 0, started: Date.now(), speed: 0,
    state: "planning", onDone,
  }) as Transfer;
  transfers.list.push(t);
  try {
    await invoke("transfer_start", {
      args: { id: t.id, connId: tab.kind === "local" ? null : tab.connId, direction, sources, destDir },
    });
  } catch (e) {
    t.state = "error";
    t.error = String(e);
  }
}

export function cancelTransfer(id: string) {
  invoke("transfer_cancel", { id }).catch(() => {});
}

export function dismissTransfer(id: string) {
  transfers.list = transfers.list.filter((t) => t.id !== id);
}

export async function initTransferEvents() {
  await listen<{ id: string; files: TFile[]; total: number }>("transfer-plan", (e) => {
    const t = byId(e.payload.id);
    if (!t) return;
    t.files = e.payload.files;
    t.total = e.payload.total;
    t.state = "running";
    t.started = Date.now();
  });
  await listen<{ id: string; index: number; fileDone: number; bytes: number }>("transfer-progress", (e) => {
    const t = byId(e.payload.id);
    if (!t) return;
    t.index = e.payload.index;
    t.fileDone = e.payload.fileDone;
    t.bytes = e.payload.bytes;
    const now = performance.now();
    const prev = lastSample.get(t.id);
    if (prev && now - prev.t > 250) {
      const inst = ((t.bytes - prev.b) / (now - prev.t)) * 1000;
      t.speed = t.speed ? t.speed * 0.7 + inst * 0.3 : inst;
      lastSample.set(t.id, { t: now, b: t.bytes });
    } else if (!prev) lastSample.set(t.id, { t: now, b: t.bytes });
  });
  await listen<{ id: string; cancelled: boolean; error: string | null }>("transfer-done", (e) => {
    const t = byId(e.payload.id);
    if (!t) return;
    lastSample.delete(t.id);
    t.state = e.payload.cancelled ? "cancelled" : e.payload.error ? "error" : "done";
    t.error = e.payload.error ?? undefined;
    if (t.state === "done") {
      t.index = t.files.length;
      t.bytes = t.total;
      toast(`${t.files.length} dosya ${t.up ? "yüklendi" : "indirildi"}`);
      setTimeout(() => dismissTransfer(t.id), 4000);
    }
    transfers.version++;
    t.onDone?.();
  });
}

export function fmtBytes(n: number) {
  if (n < 1024) return `${Math.round(n)} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}
export function fmtEta(sec: number) {
  if (!isFinite(sec) || sec < 0) return "—";
  const m = Math.floor(sec / 60), s = Math.round(sec % 60);
  return m ? `${m}:${String(s).padStart(2, "0")}` : `${s}s`;
}
