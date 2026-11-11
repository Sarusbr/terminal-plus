// Thin typed wrappers around Tauri commands. Every backend call goes through here.
import { invoke, Channel } from "@tauri-apps/api/core";

export interface ConnectArgs {
  connId: string;
  host: string;
  port: number;
  user: string;
  auth: "password" | "key";
  password?: string;
  keyPath?: string;
  passphrase?: string;
  serverId?: string;
  saveSecret?: boolean;
  proxy?: string;
}

export type ShellKind = "cmd" | "powershell" | "posix";

export interface FsEntry {
  name: string;
  kind: "d" | "f" | "l" | "o";
  size: number;
  mode: number;
  mtime: number;
  link?: string | null;
  linkDir?: boolean;
}

export interface SshConfigHost {
  name: string;
  host: string;
  port: number;
  user?: string;
  keyPath?: string;
}

export const api = {
  platform: () => invoke<string>("platform"),
  setWindowEffect: (enabled: boolean, level = "medium") => invoke<void>("set_window_effect", { enabled, level }),

  connect: (args: ConnectArgs) => invoke<{ ip: string }>("ssh_connect", { args }),
  hostkeyRespond: (connId: string, accept: boolean) =>
    invoke<void>("ssh_hostkey_respond", { connId, accept }),
  disconnect: (connId: string) => invoke<void>("ssh_disconnect", { connId }),

  ptyOpen: (connId: string, ptyId: string, cols: number, rows: number, onData: (d: Uint8Array) => void) => {
    const ch = new Channel<ArrayBuffer>();
    ch.onmessage = (buf) => onData(new Uint8Array(buf));
    return invoke<void>("pty_open", { connId, ptyId, cols, rows, onData: ch });
  },
  localInfo: () => invoke<{ shell: string; user: string }>("local_info"),
  localShells: () => invoke<{ id: string; name: string }[]>("local_shells"),
  localPtyOpen: (ptyId: string, cols: number, rows: number, shell: string | undefined, onData: (d: Uint8Array) => void) => {
    const ch = new Channel<ArrayBuffer>();
    ch.onmessage = (buf) => onData(new Uint8Array(buf));
    return invoke<{ mode: "zle" | "line" | null; kind: ShellKind }>("local_pty_open", { ptyId, cols, rows, shell, onData: ch });
  },
  /** Run a program directly, without a shell (Windows local panels). */
  localExec: (program: string, args: string[], cwd?: string, timeoutMs?: number) =>
    invoke<{ code: number; out: string; err: string }>("local_exec", { program, args, cwd, timeoutMs }),
  fsOp: (op: "mkdir" | "touch" | "rename" | "remove" | "copy" | "move", paths: string[], dest?: string) =>
    invoke<void>("fs_op", { op, paths, dest }),
  ptyWrite: (ptyId: string, data: string) => invoke<void>("pty_write", { ptyId, data }),
  ptyResize: (ptyId: string, cols: number, rows: number) =>
    invoke<void>("pty_resize", { ptyId, cols, rows }),
  ptyBusy: (ptyId: string) => invoke<boolean | null>("pty_busy", { ptyId }),
  ptyClose: (ptyId: string) => invoke<void>("pty_close", { ptyId }),
  ptyWriteSavedPassword: (connId: string, ptyId: string) =>
    invoke<boolean>("pty_write_saved_password", { connId, ptyId }),

  fileRead: (connId: string | null, path: string) =>
    invoke<{ content: string; exists: boolean }>("file_read", { target: { connId }, path }),
  fileWrite: (connId: string | null, path: string, content: string) =>
    invoke<void>("file_write", { target: { connId }, path, content }),
  tempWrite: (connId: string | null, content: string) =>
    invoke<string>("temp_write", { target: { connId }, content }),
  localListDir: (path: string) => invoke<FsEntry[]>("local_list_dir", { path }),

  secretSet: (kind: "pw" | "pp", serverId: string, secret: string) =>
    invoke<void>("secret_set", { kind, serverId, secret }),
  secretDelete: (kind: "pw" | "pp", serverId: string) =>
    invoke<void>("secret_delete", { kind, serverId }),
  secretExists: (kind: "pw" | "pp", serverId: string) =>
    invoke<boolean>("secret_exists", { kind, serverId }),

  storeLoad: <T>(name: "servers" | "settings" | "vpn") => invoke<T | null>("store_load", { name }),
  storeSave: (name: "servers" | "settings" | "vpn", data: unknown) => invoke<void>("store_save", { name, data }),
  sshConfigImport: () => invoke<SshConfigHost[]>("ssh_config_import"),
};

export const uid = () =>
  Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
