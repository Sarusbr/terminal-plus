<script setup lang="ts">
import { ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { tabs, tabLabel, closeTab, moveTab, paneHandles, openLocal } from "../stores/tabs";
import { ui } from "../stores/ui";
import Icon from "./Icon.vue";

const strip = ref<HTMLDivElement>();
const toggleMax = () => getCurrentWindow().toggleMaximize().catch(() => {});
const dragging = ref("");

function activate(id: string) {
  tabs.active = id;
  ui.page = "terminal";
  const t = tabs.list.find((x) => x.id === id);
  if (t) requestAnimationFrame(() => paneHandles.get(t.activePane)?.focus());
}

// Pointer-based reorder (HTML5 DnD conflicts with native file drop on some platforms).
function down(e: PointerEvent, id: string) {
  if (e.button !== 0 || !strip.value) return;
  const startX = e.clientX;
  const target = e.currentTarget as HTMLElement;
  let moved = false;
  const move = (ev: PointerEvent) => {
    if (!moved && Math.abs(ev.clientX - startX) < 5) return;
    if (!moved) { moved = true; dragging.value = id; target.setPointerCapture(ev.pointerId); }
    const r = strip.value!.getBoundingClientRect();
    const w = (strip.value!.querySelector(".tab") as HTMLElement).offsetWidth || r.width / tabs.list.length;
    const to = Math.max(0, Math.min(tabs.list.length - 1, Math.floor((ev.clientX - r.left) / w)));
    const from = tabs.list.findIndex((t) => t.id === id);
    moveTab(from, to);
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    dragging.value = "";
    if (!moved) activate(id);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}
</script>

<template>
  <header class="tabbar" data-tauri-drag-region>
    <div class="lights" data-tauri-drag-region />
    <div ref="strip" class="strip">
      <div
        v-for="t in tabs.list" :key="t.id" class="tab"
        :class="{ active: t.id === tabs.active && ui.page === 'terminal', current: t.id === tabs.active, dragging: dragging === t.id }"
        :title="t.kind === 'local' ? t.server.name : `${t.server.user}@${t.server.host}:${t.server.port}`"
        @pointerdown="down($event, t.id)" @auxclick.middle="closeTab(t.id)"
      >
        <span class="dot" :class="t.status" />
        <span class="label">{{ tabLabel(t) }}</span>
        <button class="close" data-tip="Kapat" @pointerdown.stop @click.stop="closeTab(t.id)">
          <Icon name="x" :size="12" />
        </button>
      </div>
      <div class="drag" data-tauri-drag-region @dblclick="toggleMax" />
    </div>
    <button class="ib add" data-tip="Yerel terminal" data-tip-pos="bottom" @click="openLocal()">
      <Icon name="plus" :size="15" />
    </button>
    <button class="ib add" data-tip="Sunucuya bağlan" data-tip-pos="bottom" @click="ui.page = 'servers'">
      <Icon name="server" :size="15" />
    </button>
  </header>
</template>

<style scoped>
.tabbar {
  height: var(--header-h); display: flex; align-items: stretch;
  border-bottom: 1px solid var(--line); user-select: none; -webkit-user-select: none;
}
.lights { width: 0; flex: none; }
.strip { flex: 1; display: flex; min-width: 0; }
.tab {
  flex: 1 1 0; min-width: 0; max-width: 240px; display: flex; align-items: center; gap: 8px;
  padding: 0 8px 0 12px; color: var(--muted); font-size: 12px; cursor: default;
  border-right: 1px solid var(--line); transition: background .1s, color .1s;
}
.tab:first-child { border-left: 1px solid var(--line); }
.tab:hover { color: var(--fg); background: rgb(var(--fg-rgb) / 0.03); }
.tab.current { color: var(--fg); }
.tab.active { background: rgb(var(--fg-rgb) / 0.06); box-shadow: inset 0 -1px 0 var(--accent); }
.tab.dragging { opacity: .7; }
.label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
.dot { width: 6px; height: 6px; border-radius: 50%; flex: none; background: var(--muted); }
.dot.connected { background: var(--ok); }
.dot.connecting { background: var(--warn); animation: pulse 1s ease-in-out infinite; }
.dot.disconnected, .dot.error { background: var(--danger); }
.dot.ended { background: var(--muted); }
@keyframes pulse { 50% { opacity: .35; } }
.close { opacity: 0; color: var(--muted); padding: 3px; border-radius: 4px; }
.tab:hover .close, .tab.current .close { opacity: 1; }
.close:hover { color: var(--fg); background: rgb(var(--fg-rgb) / 0.08); }
.drag { flex: 1 0 48px; }
.add { width: var(--header-h); height: auto; border-radius: 0; flex: none; }
</style>
