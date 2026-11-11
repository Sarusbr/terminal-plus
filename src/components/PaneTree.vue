<script setup lang="ts">
// Split layout. Panes are rendered as a flat, keyed list and absolutely positioned from
// the layout tree, so splitting/closing never remounts (and kills) an existing shell.
import { computed, ref } from "vue";
import type { PaneNode, Tab } from "../stores/tabs";
import TerminalPane from "./TerminalPane.vue";

const props = defineProps<{ tab: Tab }>();
const box = ref<HTMLDivElement>();

interface Rect { x: number; y: number; w: number; h: number }
interface Divider { node: Extract<PaneNode, { kind: "split" }>; area: Rect; at: Rect }

const geometry = computed(() => {
  const rects: Record<string, Rect> = {};
  const dividers: Divider[] = [];
  const walk = (n: PaneNode, r: Rect) => {
    if (n.kind === "leaf") { rects[n.id] = r; return; }
    if (n.dir === "row") {
      const wa = r.w * n.ratio;
      walk(n.a, { ...r, w: wa });
      walk(n.b, { ...r, x: r.x + wa, w: r.w - wa });
      dividers.push({ node: n, area: r, at: { x: r.x + wa, y: r.y, w: 0, h: r.h } });
    } else {
      const ha = r.h * n.ratio;
      walk(n.a, { ...r, h: ha });
      walk(n.b, { ...r, y: r.y + ha, h: r.h - ha });
      dividers.push({ node: n, area: r, at: { x: r.x, y: r.y + ha, w: r.w, h: 0 } });
    }
  };
  walk(props.tab.layout, { x: 0, y: 0, w: 100, h: 100 });
  return { rects, dividers };
});

const pct = (r: Rect) => ({ left: r.x + "%", top: r.y + "%", width: r.w + "%", height: r.h + "%" });

function startDrag(d: Divider, e: PointerEvent) {
  if (!box.value) return;
  const full = box.value.getBoundingClientRect();
  const target = e.currentTarget as HTMLElement;
  target.setPointerCapture(e.pointerId);
  const move = (ev: PointerEvent) => {
    const r = d.node.dir === "row"
      ? ((ev.clientX - full.left) / full.width * 100 - d.area.x) / d.area.w
      : ((ev.clientY - full.top) / full.height * 100 - d.area.y) / d.area.h;
    d.node.ratio = Math.min(0.88, Math.max(0.12, r));
  };
  const up = () => {
    target.removeEventListener("pointermove", move);
    target.removeEventListener("pointerup", up);
  };
  target.addEventListener("pointermove", move);
  target.addEventListener("pointerup", up);
}
</script>

<template>
  <div ref="box" class="layout">
    <div
      v-for="id in Object.keys(tab.panes)" :key="id" class="slot"
      :class="{ dimmed: Object.keys(tab.panes).length > 1 && tab.activePane !== id }"
      :style="geometry.rects[id] ? pct(geometry.rects[id]) : { display: 'none' }"
    >
      <TerminalPane :tab="tab" :pane-id="id" />
    </div>
    <div
      v-for="d in geometry.dividers" :key="d.node.id" class="divider" :class="d.node.dir"
      :style="pct(d.at)" @pointerdown.prevent="startDrag(d, $event)"
    />
  </div>
</template>

<style scoped>
.layout { position: absolute; inset: 0; }
.slot { position: absolute; overflow: hidden; transition: opacity .12s; }
.slot.dimmed { opacity: 0.72; }
.divider { position: absolute; background: var(--line); z-index: 2; }
.divider.row { width: 1px; cursor: col-resize; }
.divider.col { height: 1px; cursor: row-resize; }
.divider::after { content: ""; position: absolute; inset: -4px; }
</style>
