// Global UI state: active page and promise-based dialogs.
import { reactive } from "vue";

export interface PromptOpts {
  title: string;
  message?: string;
  placeholder?: string;
  secret?: boolean;
  remember?: boolean; // show "save to keychain" checkbox
  value?: string;
}
export interface PromptResult { value: string; remember: boolean }

export interface ConfirmOpts { title: string; message?: string; danger?: boolean; okText?: string }

export interface ChooseOpts { title: string; message?: string; options: string[] }

export interface HostKeyReq { connId: string; host: string; port: number; algorithm: string; fingerprint: string }

type Dialog =
  | { kind: "prompt"; opts: PromptOpts; resolve: (r: PromptResult | null) => void }
  | { kind: "confirm"; opts: ConfirmOpts; resolve: (ok: boolean) => void }
  | { kind: "hostkey"; req: HostKeyReq; resolve: (ok: boolean) => void }
  | { kind: "choose"; opts: ChooseOpts; resolve: (v: string | null) => void };

export const ui = reactive({
  page: "terminal" as string,
  dialogs: [] as Dialog[],
  toast: "" as string,
  /** Services page → Logs page hand-off. */
  logsUnit: "" as string,
});

let toastTimer: number | undefined;
export function toast(msg: string) {
  ui.toast = msg;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (ui.toast = ""), 2600);
}

export function prompt(opts: PromptOpts) {
  return new Promise<PromptResult | null>((resolve) => ui.dialogs.push({ kind: "prompt", opts, resolve }));
}
export function confirm(opts: ConfirmOpts) {
  return new Promise<boolean>((resolve) => ui.dialogs.push({ kind: "confirm", opts, resolve }));
}
export function choose(opts: ChooseOpts) {
  return new Promise<string | null>((resolve) => ui.dialogs.push({ kind: "choose", opts, resolve }));
}
export function confirmHostKey(req: HostKeyReq) {
  return new Promise<boolean>((resolve) => ui.dialogs.push({ kind: "hostkey", req, resolve }));
}
export function closeDialog(d: Dialog, result: any) {
  ui.dialogs = ui.dialogs.filter((x) => x !== d);
  (d.resolve as (r: unknown) => void)(result);
}
export type { Dialog };
