<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { sh, q as sq, isPermissionError } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import { ui, confirm } from "../../stores/ui";
import Icon from "../../components/Icon.vue";

type Source = { kind: "journal"; unit: string } | { kind: "file"; path: string };

const tab = ref<Tab | undefined>(activeTab.value);
const hasJournal = ref(false);
const units = ref<string[]>([]);
const files = ref<string[]>([]);
const filter = ref("");
const source = ref<Source | null>(null);
const text = ref("");
const lines = ref(300);
const follow = ref(true);
const search = ref("");
const sudo = ref(false);
const loading = ref(false);
const error = ref("");
const box = ref<HTMLPreElement>();
let timer = 0;
let cursor = "";
let offset = 0;
let alive = true;

const SUDO = () => (sudo.value ? "sudo -p '[sudo] password for %u: ' " : "");

async function loadSources() {
  if (!tab.value) return;
  try {
    const out = await sh(tab.value).ok(
      "echo @@J; if command -v journalctl >/dev/null 2>&1; then echo yes; " +
      "systemctl list-units --type=service --all --no-legend --plain --no-pager 2>/dev/null | awk '{print $1}'; fi; " +
      "echo @@F; find /var/log -maxdepth 2 -type f \\( -name '*.log' -o -name syslog -o -name messages -o -name dmesg " +
      "-o -name '*.err' -o -name '*_log' -o -name secure -o -name cron \\) 2>/dev/null | sort | head -300", 30000);
    const [j, f] = out.split("@@F");
    const jl = j.split("\n").map((l) => l.trim()).filter((l) => l && l !== "@@J");
    hasJournal.value = jl[0] === "yes";
    units.value = jl.slice(1).filter((u) => u.endsWith(".service"));
    files.value = f.split("\n").map((l) => l.trim()).filter(Boolean);
    if (ui.logsUnit) { open({ kind: "journal", unit: ui.logsUnit }); ui.logsUnit = ""; }
    else if (!source.value) open(hasJournal.value ? { kind: "journal", unit: "" } : files.value[0] ? { kind: "file", path: files.value[0] } : null);
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  }
}

function stop() { clearTimeout(timer); timer = 0; }

async function open(s: Source | null) {
  stop();
  source.value = s;
  text.value = "";
  error.value = "";
  cursor = "";
  offset = 0;
  if (s) await fetch(true);
}

async function fetch(initial = false) {
  const s = source.value;
  if (!s || !tab.value) return;
  loading.value = initial;
  try {
    let cmd: string;
    if (s.kind === "journal") {
      const unit = s.unit ? `-u ${sq(s.unit)} ` : "";
      cmd = `${SUDO()}journalctl ${unit}--no-pager -o short-iso --show-cursor ` + (initial || !cursor ? `-n ${lines.value}` : `--after-cursor=${sq(cursor)}`);
    } else {
      const script = initial
        ? `S=$(wc -c < "$1" | tr -d ' ') || exit 1; echo "@@SIZE $S"; tail -n ${lines.value} "$1"`
        : `S=$(wc -c < "$1" | tr -d ' ') || exit 1; O=$2; [ "$S" -lt "$O" ] && O=0; echo "@@SIZE $S"; tail -c +$((O+1)) "$1" | head -c $((S-O))`;
      cmd = `${SUDO()}sh -c ${sq(script)} _ ${sq(s.path)} ${offset}`;
    }
    const r = await sh(tab.value).run(cmd, 60000);
    if (source.value !== s) return;
    if (r.code !== 0 && !r.out.includes("-- cursor:")) {
      if (isPermissionError(r.out) && !sudo.value && initial) {
        if (await confirm({ title: "Yetki yok", message: "Bu log sudo ile okunabilir. sudo ile açılsın mı?", okText: "sudo" })) {
          sudo.value = true;
          return fetch(true);
        }
      }
      throw new Error(r.out.trim().split("\n").slice(-2).join("\n") || `exit ${r.code}`);
    }
    let out = r.out;
    if (s.kind === "journal") {
      const m = /^-- cursor: (.+)$/m.exec(out);
      if (m) cursor = m[1].trim();
      out = out.replace(/^-- cursor: .+$/m, "").replace(/^-- No entries --$/m, "").replace(/\n+$/, "");
    } else {
      const m = /^@@SIZE (\d+)\n?/.exec(out);
      if (m) { offset = Number(m[1]); out = out.slice(m[0].length); }
    }
    if (initial) text.value = out;
    else if (out.trim()) text.value += (text.value && !text.value.endsWith("\n") ? "\n" : "") + out;
    const all = text.value.split("\n");
    if (all.length > 8000) text.value = all.slice(-8000).join("\n");
    await nextTick();
    if (box.value && !search.value) box.value.scrollTop = box.value.scrollHeight;
  } catch (e) {
    if (initial) error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
  if (alive && follow.value && source.value === s) timer = window.setTimeout(() => fetch(), 2000);
}

const shown = computed(() => {
  const f = search.value.trim().toLowerCase();
  return f ? text.value.split("\n").filter((l) => l.toLowerCase().includes(f)).join("\n") : text.value;
});
const fUnits = computed(() => units.value.filter((u) => !filter.value || u.includes(filter.value.toLowerCase())));
const fFiles = computed(() => files.value.filter((p) => !filter.value || p.toLowerCase().includes(filter.value.toLowerCase())));
const isSel = (s: Source) => JSON.stringify(s) === JSON.stringify(source.value);
const title = computed(() => !source.value ? "" : source.value.kind === "file" ? source.value.path : source.value.unit || "journal (tümü)");

watch(follow, (f) => { stop(); if (f) fetch(); });
watch(sudo, () => open(source.value));
watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; source.value = null; loadSources(); });
onMounted(loadSources);
onBeforeUnmount(() => { alive = false; stop(); });
</script>

<template>
  <div class="page">
    <aside class="side">
      <div class="searchbox"><Icon name="search" :size="13" /><input v-model="filter" placeholder="kaynak" spellcheck="false" /></div>
      <div class="sources">
        <template v-if="hasJournal">
          <h4>journal</h4>
          <button class="src" :class="{ on: isSel({ kind: 'journal', unit: '' }) }" @click="open({ kind: 'journal', unit: '' })">tümü</button>
          <button v-for="u in fUnits" :key="u" class="src" :class="{ on: isSel({ kind: 'journal', unit: u }) }" @click="open({ kind: 'journal', unit: u })">{{ u.replace(/\.service$/, '') }}</button>
        </template>
        <h4>/var/log</h4>
        <button v-for="p in fFiles" :key="p" class="src" :class="{ on: isSel({ kind: 'file', path: p }) }" :title="p" @click="open({ kind: 'file', path: p })">{{ p.replace('/var/log/', '') }}</button>
        <div v-if="!files.length && !hasJournal" class="muted small">Log kaynağı bulunamadı</div>
      </div>
    </aside>
    <section class="view">
      <div class="toolbar">
        <span class="title">{{ title }}</span>
        <input v-model="search" class="mini" placeholder="ara" spellcheck="false" />
        <select v-model.number="lines" class="mini" data-tip="Satır sayısı" data-tip-pos="bottom" @change="open(source)">
          <option :value="100">100</option><option :value="300">300</option><option :value="1000">1000</option><option :value="5000">5000</option>
        </select>
        <button class="ib" :class="{ on: sudo }" :data-tip="sudo ? 'sudo açık' : 'sudo ile oku'" data-tip-pos="bottom" @click="sudo = !sudo"><Icon name="shield" /></button>
        <button class="ib" :class="{ on: follow }" :data-tip="follow ? 'Canlı takip açık' : 'Canlı takip kapalı'" data-tip-pos="bottom" @click="follow = !follow"><Icon :name="follow ? 'pause' : 'play'" /></button>
        <button class="ib" data-tip="Yeniden yükle" data-tip-pos="bottom" :class="{ spin: loading }" @click="open(source)"><Icon name="refresh" /></button>
      </div>
      <div v-if="error" class="msg err"><Icon name="alert" /> <pre>{{ error }}</pre></div>
      <pre v-else ref="box" class="log">{{ shown }}</pre>
    </section>
  </div>
</template>

<style scoped>
.page { display: flex; height: 100%; }
.side { width: 220px; flex: none; border-right: 1px solid var(--line); display: flex; flex-direction: column; min-height: 0; }
.searchbox { display: flex; align-items: center; gap: 6px; color: var(--muted); padding: 9px 10px; border-bottom: 1px solid var(--line); }
.searchbox input { flex: 1; min-width: 0; border: 0; background: transparent; padding: 2px 0; }
.sources { flex: 1; overflow: auto; padding: 4px 6px 16px; display: flex; flex-direction: column; }
h4 { flex: none; margin: 12px 6px 4px; font-size: 10.5px; font-weight: 500; color: var(--muted); text-transform: uppercase; letter-spacing: .06em; }
.src { flex: none; text-align: left; padding: 3px 8px; border-radius: 4px; font: 12px var(--font-mono); color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.src:hover { color: var(--fg); background: rgb(var(--fg-rgb) / 0.05); }
.src.on { color: var(--fg); background: rgb(var(--fg-rgb) / 0.09); }
.small { font-size: 11px; padding: 8px; }
.view { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.toolbar { display: flex; align-items: center; gap: 4px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
.title { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini { padding: 2px 6px; font-size: 11.5px; width: 110px; }
select.mini { width: auto; }
.ib.on { color: var(--fg); }
.ib.on:has(svg) { background: rgb(var(--fg-rgb) / 0.06); }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.log { flex: 1; margin: 0; overflow: auto; padding: 8px 12px; font: 12px/1.45 var(--font-mono); white-space: pre-wrap; word-break: break-all; user-select: text; -webkit-user-select: text; }
.msg { display: flex; gap: 8px; align-items: flex-start; justify-content: center; padding: 40px; }
.msg pre { margin: 0; white-space: pre-wrap; font: 12px var(--font-mono); }
.err { color: var(--danger); }
</style>
