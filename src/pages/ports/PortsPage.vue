<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { sh } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import { confirm, toast } from "../../stores/ui";
import Icon from "../../components/Icon.vue";
import { SCRIPT, parse, type Port } from "./parse";


const tab = ref<Tab | undefined>(activeTab.value);
const ports = ref<Port[]>([]);
const loading = ref(false);
const error = ref("");
const sudo = ref(false);
const q = ref("");
const source = ref("");

async function load() {
  if (!tab.value) return;
  loading.value = true;
  try {
    const r = parse(await sh(tab.value).ok(SCRIPT(sudo.value ? "sudo -p '[sudo] password for %u: ' " : ""), 60000));
    ports.value = r.ports;
    source.value = r.source;
    error.value = "";
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
}

const rows = computed(() => {
  const f = q.value.trim().toLowerCase();
  return ports.value.filter((p) => !f || String(p.port).includes(f) || p.proc?.toLowerCase().includes(f) || p.addr.includes(f));
});
const unknownProcs = computed(() => ports.value.some((p) => !p.proc));

async function kill(p: Port) {
  if (!p.pid || !tab.value) return;
  if (!(await confirm({ title: `${p.proc} (${p.pid}) sonlandırılsın mı?`, message: `${p.proto} ${p.addr}:${p.port}`, danger: true, okText: "Sonlandır" }))) return;
  try {
    await sh(tab.value).ok(`${sudo.value ? "sudo -p '[sudo] password for %u: ' " : ""}kill ${p.pid}`, 60000);
    toast(`${p.pid} sonlandırıldı`);
  } catch (e) { toast(String((e as Error)?.message ?? e)); }
  load();
}

const scope = (a: string) => (/^(127\.|::1$|localhost)/.test(a) ? "local" : a === "*" || a === "0.0.0.0" || a === "::" ? "all" : "ip");

watch(sudo, load);
watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; load(); });
onMounted(load);
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <div class="searchbox"><Icon name="search" :size="13" /><input v-model="q" placeholder="port, süreç" spellcheck="false" /></div>
      <span class="muted src">{{ source }}</span>
      <button class="ib" :class="{ on: sudo }" :data-tip="sudo ? 'sudo ile (tüm süreçler)' : 'sudo ile göster'" data-tip-pos="bottom" @click="sudo = !sudo"><Icon name="shield" /></button>
      <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
    </div>
    <div v-if="error" class="msg err"><Icon name="alert" /> {{ error }}</div>
    <div v-else class="table">
      <div v-if="unknownProcs && !sudo" class="hint muted"><Icon name="shield" :size="12" /> Bazı süreç adları için sudo gerekli</div>
      <div v-for="p in rows" :key="p.proto + p.addr + p.port + p.pid" class="row">
        <span class="proto" :class="p.proto">{{ p.proto }}</span>
        <span class="port">{{ p.port }}</span>
        <span class="addr" :class="scope(p.addr)" :data-tip="scope(p.addr) === 'local' ? 'Yalnızca yerel' : scope(p.addr) === 'all' ? 'Tüm arayüzler' : 'Belirli IP'">{{ p.addr }}</span>
        <span class="proc">{{ p.proc ?? '—' }}<span v-if="p.pid" class="muted"> · {{ p.pid }}</span></span>
        <span class="acts"><button v-if="p.pid" class="ib danger" data-tip="Süreci sonlandır" @click="kill(p)"><Icon name="x" :size="14" /></button></span>
      </div>
      <div v-if="!loading && !rows.length" class="msg muted">Dinlenen port yok</div>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 4px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
.searchbox { flex: 1; display: flex; align-items: center; gap: 6px; color: var(--muted); }
.searchbox input { flex: 1; max-width: 300px; border: 0; background: transparent; padding: 4px 0; }
.src { font-size: 11px; font-family: var(--font-mono); margin-right: 6px; }
.ib.on { color: var(--warn); }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.table { flex: 1; overflow: auto; padding: 6px 6px 16px; font-family: var(--font-mono); font-size: 12.5px; }
.hint { display: flex; gap: 6px; align-items: center; font: 11px var(--font-ui); padding: 4px 8px 8px; }
.row { display: grid; grid-template-columns: 44px 70px minmax(120px, 220px) minmax(0, 1fr) 30px; gap: 12px; align-items: center; padding: 5px 8px; border-radius: 5px; }
.row:hover { background: rgb(var(--fg-rgb) / 0.04); }
.proto { font-size: 10.5px; color: var(--muted); text-transform: uppercase; }
.proto.tcp { color: var(--accent); }
.port { text-align: right; font-weight: 500; }
.addr { color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.addr.all { color: var(--warn); }
.proc { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.acts { opacity: 0; }
.row:hover .acts { opacity: 1; }
.acts .ib { width: 24px; height: 22px; }
.msg { display: flex; gap: 8px; align-items: center; justify-content: center; padding: 50px; }
.err { color: var(--danger); }
</style>
