<script setup lang="ts">
import { computed } from "vue";
import { pages, type PageDef } from "../pages/registry";
import { activeTab, paneHandles } from "../stores/tabs";
import { ui } from "../stores/ui";
import { platform } from "../stores/settings";
import Icon from "./Icon.vue";

const top = computed(() => pages.filter((p) => !p.bottom));
const bottom = computed(() => pages.filter((p) => p.bottom));

/** Reason a page can't be opened right now, or "" when it can. */
function blocked(p: PageDef): string {
  if (!p.needsShell) return "";
  const t = activeTab.value;
  if (!t || t.status !== "connected") return "Bağlı sekme yok";
  if (t.kind === "local" && platform.os === "windows") return "Windows yerel shell'de desteklenmiyor";
  if (t.panes[t.activePane]?.alt) return "Terminalde etkileşimli program çalışıyor";
  if (t.panes[t.activePane]?.busy) return "Terminalde bir program çalışıyor";
  return "";
}

function open(p: PageDef) {
  if (blocked(p)) return;
  ui.page = p.id;
  if (p.id === "terminal") {
    const t = activeTab.value;
    if (t) requestAnimationFrame(() => paneHandles.get(t.activePane)?.focus());
  }
}
</script>

<template>
  <nav class="sidebar">
    <template v-for="group in [top, bottom]" :key="group[0]?.id">
      <div class="group">
        <button
          v-for="p in group" :key="p.id" class="ib item"
          :class="{ on: ui.page === p.id }" :disabled="!!blocked(p)"
          :data-tip="blocked(p) ? `${p.title} — ${blocked(p)}` : p.title" data-tip-pos="right"
          @click="open(p)"
        >
          <Icon :name="p.icon" :size="17" />
        </button>
      </div>
    </template>
  </nav>
</template>

<style scoped>
.sidebar {
  width: var(--bar-w); flex: none; border-right: 1px solid var(--line);
  display: flex; flex-direction: column; justify-content: space-between; padding: 6px 0;
}
.group { display: flex; flex-direction: column; align-items: center; gap: 2px; }
.item { width: 32px; height: 32px; position: relative; }
.item.on { color: var(--fg); background: rgb(var(--fg-rgb) / 0.08); }
.item.on::before {
  content: ""; position: absolute; left: -7px; top: 8px; bottom: 8px; width: 2px;
  border-radius: 2px; background: var(--accent);
}
</style>
