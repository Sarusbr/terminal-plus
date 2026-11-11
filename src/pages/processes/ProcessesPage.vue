<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { sh, isPermissionError } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import { confirm, toast } from "../../stores/ui";
import { fmtBytes } from "../../stores/transfers";
import Icon from "../../components/Icon.vue";

interface Proc { pid: number; user: string; cpu: number; mem: number; rss: number; time: string; cmd: string }
type Key = "pid" | "user" | "cpu" | "mem" | "rss" | "cmd";

const tab = ref<Tab | undefined>(activeTab.value);
const procs = ref<Proc[]>([]);
const q = ref("");
const sortKey = ref<Key>("cpu");
const desc = ref(true);
const loading = ref(false);
const error = ref("");
const auto = ref(true);
let timer = 0;
let alive = true;

// pid user %cpu %mem rss(KB) etime args — same on procps, busybox and macOS; `ps aux` as fallback.
const CMD = "ps -axo pid=,user=,%cpu=,%mem=,rss=,etime=,args= 2>/dev/null || ps aux";

function parse(out: string): Proc[] {
  const lines = out.split("\n").filter((l) => l.trim());
  const aux = lines[0]?.trim().startsWith("USER");
  const res: Proc[] = [];
  for (const l of aux ? lines.slice(1) : lines) {
    const p = l.trim().split(/\s+/);
    if (aux) {
      if (p.length < 11) continue;
      res.push({ pid: +p[1], user: p[0], cpu: +p[2], mem: +p[3], rss: +p[5] * 1024, time: p[9], cmd: p.slice(10).join(" ") });
    } else {
      if (p.length < 7 || !/^\d+$/.test(p[0])) continue;
      res.push({ pid: +p[0], user: p[1], cpu: +p[2], mem: +p[3], rss: +p[4] * 1024, time: p[5], cmd: p.slice(6).join(" ") });
    }
  }
  return res;
}

async function load() {
  clearTimeout(timer);
  if (!tab.value) return;
  loading.value = true;
  try {
    procs.value = parse(await sh(tab.value).ok(CMD, 20000));
    error.value = "";
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
  if (alive && auto.value) timer = window.setTimeout(load, 3000);
}

const rows = computed(() => {
  const f = q.value.trim().toLowerCase();
  const list = procs.value.filter((p) => !f || p.cmd.toLowerCase().includes(f) || p.user.toLowerCase().includes(f) || String(p.pid) === f);
  const k = sortKey.value, d = desc.value ? -1 : 1;
  return list.sort((a, b) => (a[k] < b[k] ? -d : a[k] > b[k] ? d : 0)).slice(0, 400);
});

function sortBy(k: Key) {
  if (sortKey.value === k) desc.value = !desc.value;
  else { sortKey.value = k; desc.value = k !== "pid" && k !== "user" && k !== "cmd"; }
}

async function kill(p: Proc, sig: "TERM" | "KILL") {
  const ok = await confirm({
    title: `${sig === "KILL" ? "kill -9" : "kill"} ${p.pid}?`,
    message: p.cmd.slice(0, 160), danger: true, okText: sig === "KILL" ? "Kill -9" : "Sonlandır",
  });
  if (!ok || !tab.value) return;
  try {
    const r = await sh(tab.value).run(`kill -${sig} ${p.pid}`);
    if (r.code !== 0) {
      if (isPermissionError(r.out) && await confirm({ title: "Yetki yok", message: "sudo ile denensin mi?", okText: "sudo" }))
        await sh(tab.value).ok(`sudo -p '[sudo] password for %u: ' kill -${sig} ${p.pid}`, 60000);
      else throw new Error(r.out.trim());
    }
    toast(`${p.pid} sonlandırıldı`);
  } catch (e) {
    toast(String((e as Error)?.message ?? e));
  }
  load();
}

watch(auto, (a) => { if (a) load(); else clearTimeout(timer); });
watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; load(); });
onMounted(load);
onBeforeUnmount(() => { alive = false; clearTimeout(timer); });
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <div class="searchbox"><Icon name="search" :size="13" /><input v-model="q" placeholder="süreç, kullanıcı, pid" spellcheck="false" /></div>
      <span class="muted count">{{ rows.length }} / {{ procs.length }}</span>
      <button class="ib" :class="{ on: auto }" :data-tip="auto ? 'Otomatik yenileme açık' : 'Otomatik yenileme kapalı'" data-tip-pos="bottom" @click="auto = !auto">
        <Icon :name="auto ? 'pause' : 'play'" />
      </button>
      <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
    </div>
    <div v-if="error && !procs.length" class="msg err"><Icon name="alert" /> {{ error }}</div>
    <div v-else class="table">
      <div class="row head">
        <button v-for="[k, l] in ([['pid', 'PID'], ['user', 'Kullanıcı'], ['cpu', 'CPU %'], ['mem', 'MEM %'], ['rss', 'RSS'], ['cmd', 'Komut']] as [Key, string][])" :key="k"
          :class="['c-' + k, { on: sortKey === k }]" @click="sortBy(k)">
          {{ l }}<Icon v-if="sortKey === k" :name="desc ? 'down' : 'up'" :size="11" />
        </button>
        <span />
      </div>
      <div v-for="p in rows" :key="p.pid" class="row">
        <span class="c-pid">{{ p.pid }}</span>
        <span class="c-user">{{ p.user }}</span>
        <span class="c-cpu" :class="{ hot: p.cpu >= 50 }">{{ p.cpu.toFixed(1) }}</span>
        <span class="c-mem" :class="{ hot: p.mem >= 20 }">{{ p.mem.toFixed(1) }}</span>
        <span class="c-rss">{{ fmtBytes(p.rss) }}</span>
        <span class="c-cmd" :title="p.cmd">{{ p.cmd }}</span>
        <span class="acts">
          <button class="ib" data-tip="Sonlandır (TERM)" @click="kill(p, 'TERM')"><Icon name="x" :size="14" /></button>
          <button class="ib danger" data-tip="kill -9" @click="kill(p, 'KILL')"><Icon name="skull" :size="14" /></button>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 4px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
.searchbox { flex: 1; display: flex; align-items: center; gap: 6px; color: var(--muted); }
.searchbox input { flex: 1; max-width: 320px; border: 0; background: transparent; padding: 4px 0; }
.count { font-size: 11px; font-family: var(--font-mono); margin-right: 6px; }
.ib.on { color: var(--fg); }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.table { flex: 1; overflow: auto; padding: 0 6px 16px; font-family: var(--font-mono); font-size: 12px; }
.row { display: grid; grid-template-columns: 64px 96px 60px 60px 74px minmax(0, 1fr) 60px; gap: 8px; align-items: center; padding: 3px 8px; border-radius: 4px; }
.row:not(.head):hover { background: rgb(var(--fg-rgb) / 0.04); }
.head { position: sticky; top: 0; z-index: 1; background: rgb(var(--bg-rgb) / 0.96); padding: 8px; font-family: var(--font-ui); font-size: 11px; }
.head button { display: inline-flex; align-items: center; gap: 3px; color: var(--muted); text-align: left; }
.head button.on { color: var(--fg); }
.c-cpu, .c-mem, .c-rss, .c-pid { text-align: right; justify-content: flex-end; }
.c-user, .c-cmd { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.c-user { color: var(--muted); }
.hot { color: var(--warn); }
.acts { display: flex; opacity: 0; justify-content: flex-end; }
.row:hover .acts { opacity: 1; }
.acts .ib { width: 24px; height: 22px; }
.msg { display: flex; gap: 8px; align-items: center; justify-content: center; padding: 50px; }
.err { color: var(--danger); }
</style>
