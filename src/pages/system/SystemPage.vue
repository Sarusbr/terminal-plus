<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { sh } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import Icon from "../../components/Icon.vue";
import { collect, collectWindows, type SysInfo } from "./collect";
import { isWinLocal } from "../../lib/winlocal";
import { fmtBytes } from "../../stores/transfers";

const tab = ref<Tab | undefined>(activeTab.value);
const info = ref<SysInfo | null>(null);
const error = ref("");
const loading = ref(false);
const interval = ref(Number(localStorage.getItem("tp.sys.interval") ?? 5));
let timer = 0;
let alive = true;

async function load() {
  clearTimeout(timer);
  if (!tab.value) return;
  loading.value = true;
  try {
    info.value = isWinLocal(tab.value) ? await collectWindows() : await collect(sh(tab.value));
    error.value = "";
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
  // Poll only while the page is open; stops on unmount.
  if (alive && interval.value > 0) timer = window.setTimeout(load, interval.value * 1000);
}

function uptime(sec: number) {
  const d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600), m = Math.floor((sec % 3600) / 60);
  return d ? `${d}g ${h}s` : h ? `${h}s ${m}dk` : `${m}dk`;
}
const pct = (a: number, b: number) => (b ? (a / b) * 100 : 0);
const level = (p: number) => (p >= 90 ? "crit" : p >= 75 ? "warn" : "");
const ring = (p: number) => `${Math.max(0, Math.min(100, p)) * 2.639} 263.9`;

watch(interval, (v) => {
  try { localStorage.setItem("tp.sys.interval", String(v)); } catch { /* ignore */ }
  load();
});
watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; info.value = null; load(); });
onMounted(load);
onBeforeUnmount(() => { alive = false; clearTimeout(timer); });
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <Icon name="gauge" class="muted" />
      <span class="host">{{ info?.host }}</span>
      <span class="muted small">{{ info?.os }}</span>
      <span class="grow" />
      <select v-model.number="interval" class="mini" data-tip="Yenileme aralığı" data-tip-pos="bottom">
        <option :value="0">elle</option><option :value="3">3 sn</option><option :value="5">5 sn</option><option :value="10">10 sn</option>
      </select>
      <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
    </div>

    <div v-if="error && !info" class="msg err"><Icon name="alert" /> {{ error }}</div>
    <div v-else-if="info" class="content">
      <div class="gauges">
        <div class="gauge" :class="level(info.cpu ?? 0)">
          <svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="42" class="track" /><circle cx="50" cy="50" r="42" class="val" :stroke-dasharray="ring(info.cpu ?? 0)" /></svg>
          <div class="center"><b>{{ info.cpu == null ? '—' : Math.round(info.cpu) + '%' }}</b><span>CPU · {{ info.cores }} çekirdek</span></div>
        </div>
        <div class="gauge" :class="level(pct(info.memUsed, info.memTotal))">
          <svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="42" class="track" /><circle cx="50" cy="50" r="42" class="val" :stroke-dasharray="ring(pct(info.memUsed, info.memTotal))" /></svg>
          <div class="center"><b>{{ Math.round(pct(info.memUsed, info.memTotal)) }}%</b><span>RAM · {{ fmtBytes(info.memUsed) }} / {{ fmtBytes(info.memTotal) }}</span></div>
        </div>
        <div class="facts">
          <div v-if="info.load.length"><span>Yük</span><b>{{ info.load.map((l) => l.toFixed(2)).join('  ') }}</b>
            <i class="mini-bar"><i :class="level(pct(info.load[0] ?? 0, info.cores))" :style="{ width: Math.min(100, pct(info.load[0] ?? 0, info.cores)) + '%' }" /></i></div>
          <div><span>Çalışma süresi</span><b>{{ uptime(info.uptime) }}</b></div>
          <div v-if="info.swapTotal"><span>Swap</span><b>{{ fmtBytes(info.swapUsed) }} / {{ fmtBytes(info.swapTotal) }}</b>
            <i class="mini-bar"><i :class="level(pct(info.swapUsed, info.swapTotal))" :style="{ width: pct(info.swapUsed, info.swapTotal) + '%' }" /></i></div>
          <div><span>Kernel</span><b>{{ info.uname }}</b></div>
        </div>
      </div>

      <h4>Diskler</h4>
      <div class="disks">
        <div v-for="d in info.disks" :key="d.mount + d.fs" class="disk">
          <div class="dl"><span class="mount">{{ d.mount }}</span><span class="muted">{{ fmtBytes(d.used) }} / {{ fmtBytes(d.size) }}</span><b :class="level(d.pct)">{{ d.pct }}%</b></div>
          <div class="bar"><i :class="level(d.pct)" :style="{ width: d.pct + '%' }" /></div>
          <div class="fs muted">{{ d.fs }}</div>
        </div>
      </div>
    </div>
    <div v-else class="msg muted">…</div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 8px; padding: 6px 10px 6px 14px; border-bottom: 1px solid var(--line); min-height: 42px; }
.host { font-family: var(--font-mono); }
.small { font-size: 11.5px; }
.grow { flex: 1; }
.mini { padding: 2px 6px; font-size: 11.5px; }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.content { flex: 1; overflow: auto; padding: 18px 20px 30px; }
.gauges { display: flex; flex-wrap: wrap; gap: 28px; align-items: center; }
.gauge { position: relative; width: 150px; height: 150px; --c: var(--accent); }
.gauge.warn { --c: var(--warn); }
.gauge.crit { --c: var(--danger); }
.gauge svg { width: 100%; height: 100%; transform: rotate(-90deg); }
.gauge circle { fill: none; stroke-width: 6; }
.track { stroke: rgb(var(--fg-rgb) / 0.07); }
.val { stroke: var(--c); stroke-linecap: round; transition: stroke-dasharray .4s ease; }
.center { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; }
.center b { font: 500 22px var(--font-mono); }
.center span { font-size: 10.5px; color: var(--muted); max-width: 110px; }
.facts { flex: 1; min-width: 240px; display: flex; flex-direction: column; gap: 10px; font-size: 12px; }
.facts > div { display: grid; grid-template-columns: 110px 1fr; gap: 3px 10px; align-items: center; }
.facts span { color: var(--muted); }
.facts b { font-weight: 400; font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini-bar { grid-column: 2; display: block; height: 3px; border-radius: 2px; background: rgb(var(--fg-rgb) / 0.07); overflow: hidden; max-width: 260px; }
.mini-bar > i, .bar > i { display: block; height: 100%; background: var(--accent); transition: width .4s; }
i.warn { background: var(--warn); }
i.crit { background: var(--danger); }
h4 { margin: 28px 0 10px; font-size: 11px; font-weight: 500; color: var(--muted); text-transform: uppercase; letter-spacing: .06em; }
.disks { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 16px 24px; }
.dl { display: flex; gap: 10px; align-items: baseline; font-size: 12px; }
.mount { flex: 1; font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.dl b { font-weight: 500; font-family: var(--font-mono); }
b.warn { color: var(--warn); }
b.crit { color: var(--danger); }
.bar { height: 5px; border-radius: 3px; background: rgb(var(--fg-rgb) / 0.07); overflow: hidden; margin: 5px 0 3px; }
.fs { font-size: 10.5px; font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.msg { display: flex; gap: 8px; align-items: center; justify-content: center; padding: 50px; }
.err { color: var(--danger); }
</style>
