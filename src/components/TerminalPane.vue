<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch, nextTick } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { api } from "../lib/api";
import { xtermTheme } from "../lib/themes";
import { currentTheme, settings } from "../stores/settings";
import { paneHandles, type Tab } from "../stores/tabs";
import { ShellRunner } from "../lib/runner";
import { openEditor } from "../stores/editor";
import Icon from "./Icon.vue";

const props = defineProps<{ tab: Tab; paneId: string }>();

const el = ref<HTMLDivElement>();
const searchInput = ref<HTMLInputElement>();
const showSearch = ref(false);
const query = ref("");
const bell = ref(false);

let term: Terminal;
let fit: FitAddon;
let searcher: SearchAddon;
let ptyId = "";
let ro: ResizeObserver;
let fitFrame = 0;
let runner: ShellRunner;
let gl: WebglAddon | null = null;
let busyTimer = 0;

/** WebGL by default; ligatures need the DOM renderer (the browser shapes text runs). */
function setRenderer() {
  if (settings.ligatures) {
    gl?.dispose();
    gl = null;
    return;
  }
  if (gl) return;
  try {
    gl = new WebglAddon();
    gl.onContextLoss(() => { gl?.dispose(); gl = null; });
    term.loadAddon(gl);
  } catch { gl = null; /* DOM renderer fallback */ }
}

/** Local shells: is a foreground program (htop, ssh, tail -f…) running? Disables panels. */
async function pollBusy() {
  if (ptyId && props.tab.kind === "local" && !runner.running) {
    const b = await api.ptyBusy(ptyId).catch(() => null);
    const p = pane();
    if (p) p.busy = b === true;
  }
  busyTimer = window.setTimeout(pollBusy, 1500);
}

const pane = () => props.tab.panes[props.paneId];

function scheduleFit() {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(() => {
    if (!el.value || el.value.clientWidth < 20 || el.value.clientHeight < 20) return;
    try { fit.fit(); } catch { /* not visible yet */ }
  });
}

async function openShell() {
  if (props.tab.status !== "connected" || !props.tab.gen) return;
  const id = `${props.paneId}-${props.tab.gen}`;
  if (ptyId === id) return;
  if (ptyId) {
    runner.dispose();
    api.ptyClose(ptyId).catch(() => {});
    term.write("\r\n\x1b[2m── yeniden bağlandı ──\x1b[0m\r\n");
  }
  ptyId = id;
  const p = pane();
  if (p) p.ptyId = id;
  scheduleFit();
  await nextTick();
  try {
    const onData = (d: Uint8Array) => { if (!runner.feed(d)) term.write(d); };
    if (props.tab.kind === "local") runner.setMode(await api.localPtyOpen(id, term.cols, term.rows, onData));
    else {
      runner.setMode(null); // remote: detected + set up on first panel use
      await api.ptyOpen(props.tab.connId, id, term.cols, term.rows, onData);
    }
  } catch (e) {
    term.write(`\r\n\x1b[31m${String(e)}\x1b[0m\r\n`);
  }
}

/** Text of the line the cursor is on, including soft-wrapped parts. */
function currentLine(): string {
  const buf = term.buffer.active;
  let y = buf.baseY + buf.cursorY;
  let text = buf.getLine(y)?.translateToString(true) ?? "";
  while (y > 0 && buf.getLine(y)?.isWrapped) {
    y--;
    text = (buf.getLine(y)?.translateToString(true) ?? "") + text;
  }
  return text;
}

/** `nano <file>` at the prompt → open the built-in editor instead. */
function interceptNano(): boolean {
  if (term.buffer.active.type === "alternate") return false;
  const m = /(?:^|[\s$#%>❯»\]])(sudo\s+(?:-\S+\s+)*)?nano((?:\s+\S+)+)\s*$/.exec(currentLine());
  if (!m) return false;
  const args = m[2].trim().split(/\s+/).filter((a) => !a.startsWith("-") && !a.startsWith("+"));
  if (args.length !== 1 || /[;&|<>`$()]/.test(args[0])) return false;
  const path = args[0].replace(/^(['"])(.*)\1$/, "$2");
  api.ptyWrite(ptyId, "\x05\x15").catch(() => {}); // clear the line in the shell
  openEditor(props.tab, path, { sudo: !!m[1] });
  return true;
}

function onInput(d: string) {
  if (!ptyId) return;
  if (runner.running) return runner.hold(d);
  if (d === "\r" && interceptNano()) return;
  api.ptyWrite(ptyId, d).catch(() => {});
}

async function copy() {
  const sel = term.getSelection();
  if (sel) await writeText(sel);
}
async function paste() {
  const text = await readText().catch(() => "");
  if (text) term.paste(text);
}

function openSearch() {
  showSearch.value = true;
  nextTick(() => searchInput.value?.select());
}
function closeSearch() {
  showSearch.value = false;
  searcher.clearDecorations();
  term.focus();
}
function find(back = false) {
  if (!query.value) return;
  const opts = { caseSensitive: false, decorations: {
    matchOverviewRuler: "#888", activeMatchColorOverviewRuler: "#fff",
    matchBackground: currentTheme().selection, activeMatchBackground: currentTheme().accent,
  } };
  back ? searcher.findPrevious(query.value, opts) : searcher.findNext(query.value, opts);
}

onMounted(() => {
  term = new Terminal({
    allowProposedApi: true,
    allowTransparency: true,
    fontFamily: settings.fontFamily,
    fontSize: settings.fontSize,
    lineHeight: settings.lineHeight,
    cursorStyle: settings.cursorStyle,
    cursorBlink: settings.cursorBlink,
    scrollback: settings.scrollback,
    theme: xtermTheme(currentTheme()),
    macOptionIsMeta: false, // Option composes characters (@, €, [ ] on TR/DE layouts)
    macOptionClickForcesSelection: true,
    rightClickSelectsWord: true,
    smoothScrollDuration: 0,
  });
  fit = new FitAddon();
  searcher = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(searcher);
  term.loadAddon(new WebLinksAddon((_e, uri) => { openUrl(uri).catch(() => {}); }));
  term.open(el.value!);
  setRenderer();

  runner = new ShellRunner(
    term,
    (d) => { if (ptyId) api.ptyWrite(ptyId, d).catch(() => {}); },
    () => (ptyId ? api.ptyBusy(ptyId).catch(() => null) : Promise.resolve(null)),
  );
  term.onData(onInput);
  term.onBinary(onInput);
  term.onResize(({ cols, rows }) => { if (ptyId) api.ptyResize(ptyId, cols, rows).catch(() => {}); });
  term.buffer.onBufferChange((b) => {
    const p = pane();
    if (p) p.alt = b.type === "alternate";
  });
  term.onBell(() => {
    if (settings.bell !== "visual") return;
    bell.value = true;
    setTimeout(() => (bell.value = false), 120);
  });
  term.textarea?.addEventListener("focus", () => (props.tab.activePane = props.paneId));

  ro = new ResizeObserver(scheduleFit);
  ro.observe(el.value!);

  paneHandles.set(props.paneId, {
    focus: () => term.focus(),
    copy, paste,
    selectAll: () => term.selectAll(),
    hasSelection: () => term.hasSelection(),
    clear: () => term.clear(),
    search: openSearch,
    write: (d) => { if (ptyId) api.ptyWrite(ptyId, d); },
    run: (cmd, opts) => runner.run(cmd, opts),
  });

  scheduleFit();
  openShell();
  if (props.tab.kind === "local") pollBusy();
  if (props.tab.activePane === props.paneId) term.focus();
});

watch(() => props.tab.gen, openShell);
watch(() => [settings.fontFamily, settings.fontSize, settings.lineHeight] as const, ([f, s, l]) => {
  term.options.fontFamily = f;
  term.options.fontSize = s;
  term.options.lineHeight = l;
  scheduleFit();
});
watch(() => [settings.theme, settings.custom], () => (term.options.theme = xtermTheme(currentTheme())), { deep: true });
watch(() => settings.ligatures, () => { setRenderer(); scheduleFit(); });
watch(() => [settings.cursorStyle, settings.cursorBlink, settings.scrollback] as const, ([c, b, n]) => {
  term.options.cursorStyle = c;
  term.options.cursorBlink = b;
  term.options.scrollback = n;
});

onBeforeUnmount(() => {
  cancelAnimationFrame(fitFrame);
  ro?.disconnect();
  paneHandles.delete(props.paneId);
  runner?.dispose();
  clearTimeout(busyTimer);
  if (ptyId) api.ptyClose(ptyId).catch(() => {});
  term?.dispose();
});

defineExpose({ refit: scheduleFit });
</script>

<template>
  <div class="pane" :class="{ active: tab.activePane === paneId, bell, liga: settings.ligatures }" @mousedown="tab.activePane = paneId">
    <div ref="el" class="xterm-host" />
    <div v-if="showSearch" class="search" @mousedown.stop>
      <Icon name="search" :size="14" />
      <input
        ref="searchInput" v-model="query" spellcheck="false" placeholder="ara"
        @keydown.enter.exact.prevent="find()" @keydown.shift.enter.prevent="find(true)"
        @keydown.esc.prevent="closeSearch" @input="find()"
      />
      <button class="ib" data-tip="Önceki" @click="find(true)"><Icon name="up" :size="14" /></button>
      <button class="ib" data-tip="Sonraki" @click="find()"><Icon name="down" :size="14" /></button>
      <button class="ib" data-tip="Kapat" @click="closeSearch"><Icon name="x" :size="14" /></button>
    </div>
  </div>
</template>

<style scoped>
.pane { position: relative; width: 100%; height: 100%; padding: 6px 4px 4px 10px; box-sizing: border-box; }
.xterm-host { width: 100%; height: 100%; }
.pane.liga :deep(.xterm-rows) { font-variant-ligatures: contextual common-ligatures; font-feature-settings: "calt" 1, "liga" 1; }
.pane.bell { box-shadow: inset 0 0 0 1px var(--accent); }
.search {
  position: absolute; top: 8px; right: 14px; z-index: 5;
  display: flex; align-items: center; gap: 4px; padding: 4px 6px;
  background: rgb(var(--bg-rgb) / 0.96); border: 1px solid var(--line); border-radius: 6px;
  color: var(--muted);
}
.search input { width: 170px; }
</style>
