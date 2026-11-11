<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import TabBar from "./components/TabBar.vue";
import SideBar from "./components/SideBar.vue";
import TabView from "./components/TabView.vue";
import Dialogs from "./components/Dialogs.vue";
import Icon from "./components/Icon.vue";
import { defineAsyncComponent } from "vue";
const EditorView = defineAsyncComponent(() => import("./editor/EditorView.vue"));
import { pages } from "./pages/registry";
import { loadSettings, platform, settings } from "./stores/settings";
import { loadServers } from "./stores/servers";
import { tabs, activeTab, paneHandles, splitPane, closePane, initTabEvents, openLocal } from "./stores/tabs";
import { ui, toast } from "./stores/ui";
import TransferDialog from "./components/TransferDialog.vue";
import { initVpnEvents, loadVpn } from "./stores/vpn";
import { initTransferEvents, startTransfer, filesCwd } from "./stores/transfers";
import { sh } from "./lib/shell";
import { isWinLocal, cwdOf } from "./lib/winlocal";
import { getCurrentWebview } from "@tauri-apps/api/webview";

const ready = ref(false);
const page = computed(() => pages.find((p) => p.id === ui.page));

onMounted(async () => {
  await Promise.all([loadSettings(), loadServers(), initTabEvents(), initTransferEvents(), initVpnEvents(), loadVpn()]);
  initDrop();
  ready.value = true;
  openLocal(); // start with a local shell, like any terminal app
  window.addEventListener("keydown", onKey, true);
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKey, true));

// ── drag & drop upload ─────────────────────────────────────
const dropping = ref(false);
function dropTarget() {
  const t = activeTab.value;
  if (!t || t.status !== "connected") return null;
  // Windows local: drop = copy into the shell's directory (reported by the prompt).
  return t;
}
async function initDrop() {
  await getCurrentWebview().onDragDropEvent(async (e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") dropping.value = !!dropTarget();
    else if (p.type === "leave") dropping.value = false;
    else if (p.type === "drop") {
      dropping.value = false;
      const t = dropTarget();
      if (!t || !p.paths.length) return;
      try {
        // Files page cwd if it's showing this tab, otherwise the shell's cwd.
        const dest = (ui.page === "files" && filesCwd[t.id])
          || (isWinLocal(t) ? cwdOf(t) ?? "" : (await sh(t).ok("pwd")).trim());
        if (!dest) return toast("Hedef klasör bilinmiyor");
        startTransfer(t, "up", p.paths, dest);
      } catch (err) {
        toast(String((err as Error)?.message ?? err));
      }
    }
  });
}

function focusActive() {
  const t = activeTab.value;
  if (t) requestAnimationFrame(() => paneHandles.get(t.activePane)?.focus());
}

function gotoTab(i: number) {
  const t = tabs.list[(i + tabs.list.length) % tabs.list.length];
  if (!t) return;
  tabs.active = t.id;
  ui.page = "terminal";
  focusActive();
}

/** Global shortcuts. macOS: Cmd. Windows/Linux: Ctrl+Shift (plain Ctrl belongs to the shell). */
function onKey(e: KeyboardEvent) {
  if (ui.dialogs.length) return;
  const mac = platform.os === "macos";
  const k = e.key.toLowerCase();
  const t = activeTab.value;
  const inTerm = ui.page === "terminal" && !!(e.target as HTMLElement)?.closest?.(".xterm");
  const pane = t && paneHandles.get(t.activePane);
  const appMod = mac
    ? e.metaKey && !e.ctrlKey && !e.altKey
    : e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
  const plainMod = mac ? e.metaKey && !e.ctrlKey && !e.altKey : e.ctrlKey && !e.altKey && !e.metaKey;

  let act: (() => void) | null = null;

  if (appMod) {
    switch (k) {
      case "t": act = () => (mac && e.shiftKey ? (ui.page = "servers") : openLocal()); break;
      case "o": if (mac) act = () => (ui.page = "servers"); break;
      case "w": if (t && ui.page === "terminal") act = () => closePane(t, t.activePane); break;
      case "d": if (t && ui.page === "terminal" && t.status === "connected")
        act = () => splitPane(t, t.activePane, mac && e.shiftKey ? "col" : "row"); break;
      case "e": if (!mac && t && ui.page === "terminal" && t.status === "connected")
        act = () => splitPane(t, t.activePane, "col"); break;
      case "c": if (inTerm && pane) act = () => pane.copy(); break;
      case "v": if (inTerm && pane) act = () => pane.paste(); break;
      case "a": if (inTerm && pane) act = () => pane.selectAll(); break;
      case "f": if (pane && ui.page === "terminal") act = () => pane.search(); break;
      case "k": if (pane && ui.page === "terminal") act = () => pane.clear(); break;
      case "[": case "{": if (mac && e.shiftKey) act = () => gotoTab(tabs.list.findIndex((x) => x.id === tabs.active) - 1); break;
      case "]": case "}": if (mac && e.shiftKey) act = () => gotoTab(tabs.list.findIndex((x) => x.id === tabs.active) + 1); break;
    }
  }
  if (!act && plainMod && !e.shiftKey) {
    if (/^[1-9]$/.test(e.key)) act = () => gotoTab(Number(e.key) === 9 ? tabs.list.length - 1 : Number(e.key) - 1);
    else if (k === "," ) act = () => (ui.page = "settings");
    else if (k === "=" || k === "+") act = () => (settings.fontSize = Math.min(32, settings.fontSize + 1));
    else if (k === "-") act = () => (settings.fontSize = Math.max(8, settings.fontSize - 1));
    else if (k === "0") act = () => (settings.fontSize = 13);
  }
  // Windows/Linux: Ctrl+C copies when there is a selection, otherwise goes to the shell as ^C.
  if (!act && !mac && inTerm && pane && e.ctrlKey && !e.shiftKey && !e.altKey && k === "c" && pane.hasSelection()) {
    act = () => pane.copy();
  }
  if (!act && !mac && e.ctrlKey && k === "tab") {
    act = () => gotoTab(tabs.list.findIndex((x) => x.id === tabs.active) + (e.shiftKey ? -1 : 1));
  }
  if (!act && k === "escape" && ui.page !== "terminal" && !(e.target as HTMLElement)?.closest?.("input,select,textarea,form")) {
    act = () => { ui.page = "terminal"; focusActive(); };
  }

  if (act) {
    e.preventDefault();
    e.stopPropagation();
    act();
  }
}
</script>

<template>
  <div v-if="ready" class="app">
    <TabBar />
    <div class="body">
      <SideBar />
      <main class="content">
        <div v-show="ui.page === 'terminal'" class="terms">
          <TabView v-for="t in tabs.list" v-show="t.id === tabs.active" :key="t.id" :tab="t" />
          <div v-if="!tabs.list.length" class="welcome">
            <Icon name="terminal" :size="30" />
            <div class="welcome-actions">
              <button class="btn primary" @click="openLocal()"><Icon name="terminal" :size="14" /> Yerel terminal</button>
              <button class="btn" @click="ui.page = 'servers'"><Icon name="server" :size="14" /> Sunucuya bağlan</button>
            </div>
          </div>
        </div>
        <div v-if="ui.page === 'editor'" class="pagehost">
          <EditorView />
        </div>
        <div v-else-if="page?.component" class="pagehost">
          <component :is="page.component" />
        </div>
      </main>
    </div>
    <Dialogs />
    <TransferDialog />
    <transition name="fade">
      <div v-if="dropping" class="dropzone">
        <Icon name="pull" :size="26" />
        <div>{{ activeTab?.kind === 'local' ? 'Buraya kopyala' : `${activeTab?.server.host} sunucusuna yükle` }}</div>
      </div>
    </transition>
    <transition name="fade"><div v-if="ui.toast" class="toast">{{ ui.toast }}</div></transition>
  </div>
</template>

<style>
.app {
  height: 100%; display: flex; flex-direction: column;
  background: rgb(var(--bg-rgb) / var(--bg-opacity));
}
.body { flex: 1; display: flex; min-height: 0; }
.content { flex: 1; position: relative; min-width: 0; }
.terms, .pagehost { position: absolute; inset: 0; }
.pagehost { animation: pagein .12s ease-out; }
@keyframes pagein { from { opacity: 0; transform: translateY(3px); } }
.welcome { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 16px; color: var(--muted); }
.dropzone {
  position: fixed; inset: 0; z-index: 80; display: flex; flex-direction: column; gap: 10px;
  align-items: center; justify-content: center; pointer-events: none; color: var(--fg);
  background: rgb(var(--bg-rgb) / 0.7); box-shadow: inset 0 0 0 2px var(--accent);
}
.welcome-actions { display: flex; gap: 8px; }
.toast {
  position: fixed; bottom: 16px; left: 50%; transform: translateX(-50%); z-index: 120;
  padding: 7px 14px; border-radius: 6px; font-size: 12px;
  background: rgb(var(--bg-rgb) / 0.97); border: 1px solid var(--line);
}
.fade-enter-active, .fade-leave-active { transition: opacity .12s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
</style>
