// Page-facing command service. Pages call `sh(tab).run(...)`; this picks the tab's
// active pane runner and supplies the sudo password flow.
import { api } from "./api";
import { paneHandles, type Tab } from "../stores/tabs";
import { prompt } from "../stores/ui";
import { RunError, type RunResult } from "./runner";

export { q } from "./runner";

export interface Shell {
  tab: Tab;
  /** Run and return output; rejects on runner errors (not on non-zero exit). */
  run(cmd: string, timeout?: number): Promise<RunResult>;
  /** Run, throw with output when exit code != 0. */
  ok(cmd: string, timeout?: number): Promise<string>;
  /** Type a visible command into the terminal (e.g. docker exec -it). */
  type(cmd: string): void;
}

const secretId = (tab: Tab) => (tab.kind === "local" ? "local" : tab.server.id);

/** Answer a sudo prompt: saved keychain password first, else ask. */
async function sudoPassword(tab: Tab, ptyId: string, attempt: { n: number }): Promise<boolean> {
  const connId = tab.kind === "local" ? "local" : tab.connId;
  if (attempt.n++ === 0 && (await api.ptyWriteSavedPassword(connId, ptyId).catch(() => false))) return true;
  const r = await prompt({
    title: "sudo",
    message: `${tab.kind === "local" ? "Bu bilgisayar" : `${tab.server.user}@${tab.server.host}`} için şifre`,
    placeholder: "şifre", secret: true, remember: true,
  });
  if (!r) return false;
  await api.ptyWrite(ptyId, r.value + "\n");
  if (r.remember) api.secretSet("pw", secretId(tab), r.value).catch(() => {});
  return true;
}

export function sh(tab: Tab): Shell {
  const pane = () => {
    const h = paneHandles.get(tab.activePane);
    const ptyId = tab.panes[tab.activePane]?.ptyId;
    if (!h || !ptyId || tab.status !== "connected") throw new RunError("CLOSED", "Terminal bağlı değil");
    return { h, ptyId };
  };
  const self: Shell = {
    tab,
    run(cmd, timeout) {
      const { h, ptyId } = pane();
      const attempt = { n: 0 };
      return h.run(cmd, { timeout, onPassword: () => sudoPassword(tab, ptyId, attempt) });
    },
    async ok(cmd, timeout) {
      const r = await self.run(cmd, timeout);
      if (r.code !== 0) throw new Error(r.out.trim().split("\n").slice(-3).join("\n") || `exit ${r.code}`);
      return r.out;
    },
    type(cmd) {
      const { h } = pane();
      h.write("\x05\x15" + cmd + "\r");
    },
  };
  return self;
}

export const isPermissionError = (s: string) => /permission denied|operation not permitted|EACCES/i.test(s);
