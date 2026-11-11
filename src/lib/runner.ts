// Shell command runner — the single place panel commands are sent from.
//
// Commands are typed into the pane's own PTY shell (same cwd, env, sudo session),
// wrapped in unique markers. While a command runs, PTY output is diverted away from
// xterm, so the terminal never shows panel traffic. Afterwards the old prompt lines are
// erased and the shell's fresh prompt is drawn in their place.

import type { Terminal } from "@xterm/xterm";

export interface RunOpts {
  timeout?: number;
  /** Called when sudo asks for a password; must type it (or return false to abort). */
  onPassword?: () => Promise<boolean>;
}
export interface RunResult { out: string; code: number }

export class RunError extends Error {
  constructor(public code: "ALT_SCREEN" | "BUSY" | "TIMEOUT" | "CLOSED" | "NO_START" | "CANCELLED", msg?: string) {
    super(msg ?? code);
  }
}

interface Job {
  id: string;
  cmd: string;
  opts: RunOpts;
  resolve: (r: RunResult) => void;
  reject: (e: Error) => void;
}

type Phase = "idle" | "start" | "body" | "post";

/**
 * How commands reach the shell without landing in its history:
 * - "zle":  zsh. A key sequence switches ZLE into a private keymap that collects the
 *           command into a variable (nothing is drawn, nothing goes to history) and
 *           runs it on \x1e. Installed by the local shell integration or by BOOT.
 * - "line": bash/sh. Typed as a space-prefixed line with HISTCONTROL=ignorespace.
 */
export type RunMode = "zle" | "line";

const ZLE_TRIGGER = "\x1b[57401~";
const ZLE_END = "\x1e";
// eslint-disable-next-line no-control-regex
const ZLE_REDRAW = /\r+(?:\x1b\[[0-9;?]*[A-Za-z])*\x1b\[J[^\r\n]*/g;

/** zsh widget setup (same as the local integration file). */
export const ZSH_WIDGETS =
  "_tp_begin() { _tp_cmd=; zle -K _tpmap; }; _tp_acc() { _tp_cmd+=$KEYS; }; " +
  "_tp_run() { zle -K main; local c=$_tp_cmd; _tp_cmd=; eval \"$c\"; zle reset-prompt; zle -R; }; " +
  "zle -N _tp_begin; zle -N _tp_acc; zle -N _tp_run; bindkey -N _tpmap; " +
  "bindkey -M _tpmap -R '^@'-'^?' _tp_acc; bindkey -M _tpmap -R '\\M-^@'-'\\M-^?' _tp_acc; " +
  "bindkey -M _tpmap '^^' _tp_run; for _tp_m in emacs viins vicmd; do bindkey -M $_tp_m '^[[57401~' _tp_begin; done; unset _tp_m";

/**
 * zsh has no "delete history entry" builtin: rebuild the in-memory list without this
 * setup line (fc -W → filter → fc -p → fc -R) and strip it from $HISTFILE if it was
 * already appended there.
 */
const ZSH_FORGET =
  `{ _tp_f=$(mktemp) && fc -W "$_tp_f" && grep -v '__TP_MODE_' "$_tp_f" > "$_tp_f.2"; ` +
  `_tp_hf=$HISTFILE; _tp_hs=$HISTSIZE; _tp_ss=$SAVEHIST; fc -p; HISTFILE=$_tp_hf; HISTSIZE=$_tp_hs; SAVEHIST=$_tp_ss; ` +
  `fc -R "$_tp_f.2"; if [ -n "$HISTFILE" ] && [ -w "$HISTFILE" ] && grep -q '__TP_MODE_' "$HISTFILE"; then ` +
  `grep -v '__TP_MODE_' "$HISTFILE" > "$_tp_f"; cat "$_tp_f" > "$HISTFILE"; fi; rm -f "$_tp_f" "$_tp_f.2"; ` +
  `unset _tp_f _tp_hf _tp_hs _tp_ss; } 2>/dev/null`;

/** One-time setup for remote shells (sent on first panel use, never on connect). */
const BOOT =
  `if [ -n "$ZSH_VERSION" ]; then ${ZSH_WIDGETS}; ${ZSH_FORGET}; echo __TP_MODE_zle; ` +
  `elif [ -n "$BASH_VERSION" ]; then HISTCONTROL="ignorespace\${HISTCONTROL:+:$HISTCONTROL}"; ` +
  `case "$(history 1)" in *__TP_MODE_*) history -d "$(history 1 | awk '{print $1}')";; esac; echo __TP_MODE_line; ` +
  `else echo __TP_MODE_line; fi`;

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>]|\x1b[78]/g;

export const stripAnsi = (s: string) => s.replace(ANSI, "");

/** POSIX single-quote escaping for file names and arguments. */
export const q = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

export class ShellRunner {
  private queue: Job[] = [];
  private job: Job | null = null;
  private phase: Phase = "idle";
  private text = "";
  private post = "";
  private postTimer = 0;
  private hardTimer = 0;
  private held: string[] = [];
  private decoder = new TextDecoder();
  private pwPrompts = 0;
  private pwBusy = false;
  private mode: RunMode | null = null;
  private boot: Promise<void> | null = null;

  constructor(
    private term: Terminal,
    private write: (data: string) => void,
    private isBusy: () => Promise<boolean | null>,
  ) {}

  get running() { return this.phase !== "idle"; }

  /** Known up front for local shells started with our integration. */
  setMode(mode: RunMode | null) {
    this.mode = mode;
    this.boot = null;
  }

  run(cmd: string, opts: RunOpts = {}): Promise<RunResult> {
    if (this.mode === null && !this.boot) {
      this.mode = "line"; // the boot line itself goes in line mode
      this.boot = this.enqueue(BOOT, {}).then(
        (r) => { this.mode = /__TP_MODE_zle/.test(r.out) ? "zle" : "line"; },
        () => { this.boot = null; this.mode = null; },
      );
    }
    return this.boot ? this.boot.then(() => this.enqueue(cmd, opts)) : this.enqueue(cmd, opts);
  }

  private enqueue(cmd: string, opts: RunOpts): Promise<RunResult> {
    return new Promise((resolve, reject) => {
      this.queue.push({ id: Math.random().toString(36).slice(2, 10), cmd, opts, resolve, reject });
      if (this.phase === "idle") this.next();
    });
  }

  /** Keystrokes typed while a panel command runs are held and replayed afterwards. */
  hold(data: string) { this.held.push(data); }

  /** Route PTY output. Returns true when consumed (not for display). */
  feed(bytes: Uint8Array): boolean {
    if (this.phase === "idle") return false;
    const chunk = this.decoder.decode(bytes, { stream: true });
    if (this.phase === "post") {
      this.post += chunk;
      this.armPost(60);
      return true;
    }
    this.text += chunk;
    this.scan();
    return true;
  }

  dispose() {
    for (const j of [this.job, ...this.queue]) j?.reject(new RunError("CLOSED"));
    this.queue = [];
    this.job = null;
    this.phase = "idle";
    clearTimeout(this.postTimer);
    clearTimeout(this.hardTimer);
  }

  private async next(): Promise<void> {
    const job = this.queue.shift();
    if (!job) return;
    if (this.term.buffer.active.type === "alternate") {
      job.reject(new RunError("ALT_SCREEN", "Terminalde etkileşimli program çalışıyor"));
      return this.next();
    }
    if ((await this.isBusy()) === true) {
      job.reject(new RunError("BUSY", "Terminalde bir program çalışıyor"));
      return this.next();
    }
    this.job = job;
    this.phase = "start";
    this.text = "";
    this.post = "";
    this.pwPrompts = 0;
    const id = job.id;
    // printf '%s' with the id as an argument → the echoed input never contains the marker.
    const body = `printf '\\n__TP_%s_S__\\n' ${id}; { ${job.cmd}; } 2>&1; printf '\\n__TP_%s_E_%s__\\n' ${id} "$?"`;
    if (this.mode === "zle") this.write(ZLE_TRIGGER + body + ZLE_END);
    // Ctrl+E Ctrl+U drops half-typed input; the leading space keeps it out of history.
    else this.write("\x05\x15 " + body + "\r");
    const timeout = job.opts.timeout ?? 20000;
    this.hardTimer = window.setTimeout(() => this.abort(new RunError("TIMEOUT", "Komut zaman aşımına uğradı")), timeout);
  }

  private scan() {
    const job = this.job!;
    if (this.phase === "start") {
      const m = this.text.indexOf(`__TP_${job.id}_S__`);
      if (m < 0) return;
      const nl = this.text.indexOf("\n", m);
      if (nl < 0) return;
      this.text = this.text.slice(nl + 1);
      this.phase = "body";
    }
    if (this.phase === "body") {
      const re = new RegExp(`__TP_${job.id}_E_(\\d+)__`);
      const m = re.exec(this.text);
      if (m) {
        let out = this.text.slice(0, m.index);
        const after = this.text.slice(m.index + m[0].length).replace(/^\r?\n/, "");
        // zle can flush a deferred prompt redraw into the next command's output.
        if (this.mode === "zle") out = out.replace(ZLE_REDRAW, "");
        out = stripAnsi(out).replace(/\r/g, "").replace(/\n$/, "");
        clearTimeout(this.hardTimer);
        this.job = null;
        this.phase = "post";
        this.post = after;
        this.armPost(80);
        job.resolve({ out, code: Number(m[1]) });
        return;
      }
      this.checkSudo();
    }
  }

  private checkSudo() {
    const tail = stripAnsi(this.text.slice(-200));
    if (this.pwBusy || !/(\[sudo\] password for [^:\n]*:|^Password:|parola|şifre)[^\n]*$/im.test(tail)) return;
    const job = this.job!;
    if (++this.pwPrompts > 2 || !job.opts.onPassword) {
      this.write("\x03");
      return;
    }
    this.pwBusy = true;
    this.text = this.text.slice(0, -tail.length) + tail.replace(/[^\n]*$/, "");
    job.opts.onPassword().then((ok) => {
      this.pwBusy = false;
      if (!ok) this.write("\x03");
    });
  }

  private abort(err: Error) {
    const job = this.job;
    if (!job) return;
    this.write("\x03");
    this.job = null;
    this.phase = "post";
    this.post = "";
    this.armPost(300);
    job.reject(err);
  }

  private armPost(ms: number) {
    clearTimeout(this.postTimer);
    this.postTimer = window.setTimeout(() => this.finishPost(), ms);
  }

  /** Replace the old prompt with the shell's fresh one, then resume normal display. */
  private finishPost() {
    if (this.mode === "zle") {
      // ZLE never saw any output (we hid it), so its redraw is already correct.
      this.term.write(this.post);
      return this.resume();
    }
    const prompt = this.post.replace(/^[\r\n]+/, "");
    const lines = (prompt.match(/\n/g) ?? []).length;
    let seq = "\r\x1b[2K";
    for (let i = 0; i < lines; i++) seq += "\x1b[1A\x1b[2K";
    this.term.write(seq + prompt);
    this.resume();
  }

  private resume() {
    this.post = "";
    this.phase = "idle";
    const held = this.held.join("");
    this.held = [];
    if (held) this.write(held);
    if (this.queue.length) this.next();
  }
}
