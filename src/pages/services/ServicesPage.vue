<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { sh, q as sq } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import { confirm, toast, ui } from "../../stores/ui";
import Icon from "../../components/Icon.vue";

interface Svc { unit: string; load: string; active: string; sub: string; desc: string; enabled: string }

const tab = ref<Tab | undefined>(activeTab.value);
const svcs = ref<Svc[]>([]);
const loading = ref(false);
const error = ref("");
const missing = ref(false);
const q = ref("");
const only = ref<"all" | "active" | "failed">("all");
const busy = ref<Set<string>>(new Set());
const status = ref<{ unit: string; text: string } | null>(null);

const SCRIPT =
  "command -v systemctl >/dev/null 2>&1 || { echo @@NOSYSTEMD; exit 0; }; " +
  "echo @@UNITS; systemctl list-units --type=service --all --no-legend --plain --no-pager; " +
  "echo @@FILES; systemctl list-unit-files --type=service --no-legend --no-pager";

async function load() {
  if (!tab.value) return;
  loading.value = true;
  try {
    // `exit` would close the user's shell — guard with a subshell.
    const out = await sh(tab.value).ok(`( ${SCRIPT} )`, 30000);
    missing.value = out.includes("@@NOSYSTEMD");
    if (missing.value) { svcs.value = []; return; }
    const [u, f] = out.split("@@FILES");
    const enabled = new Map<string, string>();
    for (const l of (f ?? "").split("\n")) { const p = l.trim().split(/\s+/); if (p[0]?.endsWith(".service")) enabled.set(p[0], p[1] ?? ""); }
    const list: Svc[] = [];
    for (const l of u.replace("@@UNITS", "").split("\n")) {
      const p = l.trim().replace(/^●\s*/, "").split(/\s+/);
      if (!p[0]?.endsWith(".service") || p.length < 4) continue;
      list.push({ unit: p[0], load: p[1], active: p[2], sub: p[3], desc: p.slice(4).join(" "), enabled: enabled.get(p[0]) ?? "" });
    }
    svcs.value = list;
    error.value = "";
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
}

const rows = computed(() => {
  const f = q.value.trim().toLowerCase();
  return svcs.value
    .filter((s) => only.value === "all" || (only.value === "active" ? s.active === "active" : s.active === "failed"))
    .filter((s) => !f || s.unit.toLowerCase().includes(f) || s.desc.toLowerCase().includes(f))
    .sort((a, b) => Number(b.active === "failed") - Number(a.active === "failed") || a.unit.localeCompare(b.unit));
});

/** Try without privileges (no polkit prompt), fall back to sudo. */
async function act(s: Svc, verb: "start" | "stop" | "restart" | "enable" | "disable") {
  if ((verb === "stop" || verb === "disable") && !(await confirm({ title: `${s.unit} ${verb}?`, danger: true, okText: verb })))
    return;
  busy.value = new Set(busy.value).add(s.unit);
  try {
    const u = sq(s.unit);
    let r = await sh(tab.value!).run(`systemctl --no-ask-password ${verb} ${u}`, 60000);
    if (r.code !== 0 && /access denied|authentication|permission|interactive/i.test(r.out))
      r = await sh(tab.value!).run(`sudo -p '[sudo] password for %u: ' systemctl ${verb} ${u}`, 90000);
    if (r.code !== 0) throw new Error(r.out.trim().split("\n").slice(-2).join("\n"));
    toast(`${s.unit}: ${verb}`);
  } catch (e) {
    toast(String((e as Error)?.message ?? e));
  } finally {
    const b = new Set(busy.value); b.delete(s.unit); busy.value = b;
  }
  await load();
  if (status.value?.unit === s.unit) showStatus(s);
}

async function showStatus(s: Svc) {
  status.value = { unit: s.unit, text: "…" };
  const r = await sh(tab.value!).run(`systemctl status ${sq(s.unit)} --no-pager -l -n 40`, 30000);
  if (status.value?.unit === s.unit) status.value.text = r.out;
}

function logs(s: Svc) {
  ui.logsUnit = s.unit;
  ui.page = "logs";
}

watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; status.value = null; load(); });
onMounted(load);
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <div class="searchbox"><Icon name="search" :size="13" /><input v-model="q" placeholder="servis" spellcheck="false" /></div>
      <div class="seg">
        <button :class="{ on: only === 'all' }" @click="only = 'all'">tümü</button>
        <button :class="{ on: only === 'active' }" @click="only = 'active'">aktif</button>
        <button :class="{ on: only === 'failed' }" @click="only = 'failed'">hatalı</button>
      </div>
      <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
    </div>
    <div v-if="missing" class="msg muted"><Icon name="services" :size="22" /> Bu sistemde systemd yok</div>
    <div v-else-if="error" class="msg err"><Icon name="alert" /> {{ error }}</div>
    <div v-else class="body">
      <div class="list">
        <div v-for="s in rows" :key="s.unit" class="row" :class="{ busy: busy.has(s.unit), sel: status?.unit === s.unit }">
          <span class="dot" :class="s.active" :data-tip="`${s.active} (${s.sub})`" />
          <div class="main">
            <div class="name">{{ s.unit.replace(/\.service$/, '') }}</div>
            <div class="desc">{{ s.desc }}</div>
          </div>
          <span class="en" :class="s.enabled">{{ s.enabled }}</span>
          <div class="acts">
            <button v-if="s.active !== 'active'" class="ib" data-tip="Başlat" @click="act(s, 'start')"><Icon name="play" /></button>
            <button v-else class="ib" data-tip="Durdur" @click="act(s, 'stop')"><Icon name="stop" /></button>
            <button class="ib" data-tip="Yeniden başlat" @click="act(s, 'restart')"><Icon name="refresh" /></button>
            <button v-if="s.enabled === 'disabled'" class="ib" data-tip="Açılışta başlat (enable)" @click="act(s, 'enable')"><Icon name="check" /></button>
            <button v-else-if="s.enabled === 'enabled'" class="ib" data-tip="Açılışta başlatma (disable)" @click="act(s, 'disable')"><Icon name="x" /></button>
            <button class="ib" data-tip="Durum" @click="showStatus(s)"><Icon name="info" /></button>
            <button class="ib" data-tip="Loglar" @click="logs(s)"><Icon name="scroll" /></button>
          </div>
        </div>
      </div>
      <aside v-if="status" class="detail">
        <div class="dhead"><Icon name="info" /><span class="title">{{ status.unit }}</span><button class="ib" data-tip="Kapat" @click="status = null"><Icon name="x" /></button></div>
        <pre class="log">{{ status.text }}</pre>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
.searchbox { flex: 1; display: flex; align-items: center; gap: 6px; color: var(--muted); }
.searchbox input { flex: 1; max-width: 300px; border: 0; background: transparent; padding: 4px 0; }
.seg { display: inline-flex; border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
.seg button { padding: 3px 10px; font-size: 11.5px; color: var(--muted); }
.seg button.on { color: var(--fg); background: rgb(var(--fg-rgb) / 0.08); }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.body { flex: 1; display: flex; min-height: 0; }
.list { flex: 1; overflow: auto; padding: 4px 6px 16px; min-width: 0; }
.row { display: flex; align-items: center; gap: 10px; padding: 5px 8px; border-radius: 6px; }
.row:hover, .row.sel { background: rgb(var(--fg-rgb) / 0.04); }
.row.busy { opacity: .5; pointer-events: none; }
.dot { width: 7px; height: 7px; border-radius: 50%; flex: none; background: var(--muted); opacity: .6; }
.dot.active { background: var(--ok); opacity: 1; }
.dot.failed { background: var(--danger); opacity: 1; }
.dot.activating, .dot.deactivating, .dot.reloading { background: var(--warn); opacity: 1; }
.main { flex: 1; min-width: 0; }
.name { font: 12.5px var(--font-mono); }
.desc { font-size: 11px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.en { font-size: 10.5px; color: var(--muted); }
.en.enabled { color: var(--accent); }
.acts { display: flex; opacity: 0; }
.row:hover .acts, .row.sel .acts { opacity: 1; }
.detail { width: min(50%, 560px); flex: none; border-left: 1px solid var(--line); display: flex; flex-direction: column; min-height: 0; }
.dhead { display: flex; align-items: center; gap: 6px; padding: 6px 8px; border-bottom: 1px solid var(--line); color: var(--muted); }
.dhead .title { flex: 1; color: var(--fg); font-family: var(--font-mono); font-size: 12.5px; }
.log { flex: 1; margin: 0; overflow: auto; padding: 8px 10px; font: 11.5px/1.45 var(--font-mono); white-space: pre-wrap; word-break: break-all; user-select: text; -webkit-user-select: text; }
.msg { display: flex; gap: 8px; align-items: center; justify-content: center; padding: 50px; }
.err { color: var(--danger); }
</style>
