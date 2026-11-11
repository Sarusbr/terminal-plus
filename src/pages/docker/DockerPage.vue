<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { activeTab, type Tab } from "../../stores/tabs";
import { confirm, prompt, toast, ui } from "../../stores/ui";
import Icon from "../../components/Icon.vue";
import { sh } from "../../lib/shell";
import { docker, DockerError, type Container, type Image, type Volume, type Network, type Compose, type Stats } from "./docker";

type Section = "containers" | "images" | "volumes" | "networks" | "compose";
type Detail = { kind: "logs" | "inspect" | "stats"; c: Container };

const tab = ref<Tab | undefined>(activeTab.value);
const section = ref<Section>("containers");
const loading = ref(false);
const error = ref<{ kind: string; msg: string } | null>(null);
const busy = ref<Set<string>>(new Set());
const filter = ref("");

const containers = ref<Container[]>([]);
const images = ref<Image[]>([]);
const volumes = ref<Volume[]>([]);
const networks = ref<Network[]>([]);
const composes = ref<Compose[]>([]);

const detail = ref<Detail | null>(null);
const logText = ref("");
const logLines = ref(200);
const logFollow = ref(true);
const logSearch = ref("");
const inspectRaw = ref("");
const inspectView = ref<"summary" | "raw">("summary");
const stats = ref<Stats | null>(null);
const logBox = ref<HTMLPreElement>();
let pollTimer = 0;
let logSince = 0;

const d = () => docker(tab.value!);

const sections: { id: Section; icon: string; tip: string }[] = [
  { id: "containers", icon: "box", tip: "Container'lar" },
  { id: "images", icon: "layers", tip: "Image'lar" },
  { id: "volumes", icon: "database", tip: "Volume'ler" },
  { id: "networks", icon: "network", tip: "Network'ler" },
  { id: "compose", icon: "list", tip: "Compose projeleri" },
];

const match = (...v: (string | undefined)[]) => {
  const f = filter.value.trim().toLowerCase();
  return !f || v.some((x) => x?.toLowerCase().includes(f));
};
const shownContainers = computed(() =>
  containers.value.filter((c) => match(c.Names, c.Image, c.ID))
    .sort((a, b) => Number(b.State === "running") - Number(a.State === "running") || a.Names.localeCompare(b.Names)));

async function load() {
  if (!tab.value) return;
  loading.value = true;
  error.value = null;
  try {
    const x = d();
    if (section.value === "containers") containers.value = x.jsonLines<Container>(await x.run("ps -a --format '{{json .}}'"));
    if (section.value === "images") images.value = x.jsonLines<Image>(await x.run("images --format '{{json .}}'"));
    if (section.value === "volumes") volumes.value = x.jsonLines<Volume>(await x.run("volume ls --format '{{json .}}'"));
    if (section.value === "networks") networks.value = x.jsonLines<Network>(await x.run("network ls --format '{{json .}}'"));
    if (section.value === "compose") {
      const out = (await x.run("compose ls -a --format json")).trim();
      composes.value = out.startsWith("[") ? JSON.parse(out) : [];
    }
  } catch (e) {
    error.value = e instanceof DockerError ? { kind: e.kind, msg: e.message } : { kind: "other", msg: String((e as Error)?.message ?? e) };
  } finally {
    loading.value = false;
  }
}

async function doAct(key: string, args: string, msg?: string) {
  busy.value = new Set(busy.value).add(key);
  try {
    await d().run(args, 120000);
    if (msg) toast(msg);
  } catch (e) {
    toast(String((e as Error)?.message ?? e));
  } finally {
    const b = new Set(busy.value); b.delete(key); busy.value = b;
  }
  await load();
}

const cAct = (c: Container, verb: string) => doAct(c.ID, `${verb} ${d().q(c.ID)}`);
async function cRemove(c: Container) {
  if (await confirm({ title: `${c.Names} silinsin mi?`, message: "Container zorla kaldırılır (rm -f).", danger: true, okText: "Sil" }))
    doAct(c.ID, `rm -f ${d().q(c.ID)}`);
}
async function cKill(c: Container) {
  if (await confirm({ title: `${c.Names} kill?`, message: "SIGKILL gönderilir.", danger: true, okText: "Kill" }))
    cAct(c, "kill");
}
function cExec(c: Container) {
  d().exec(c.ID);
  ui.page = "terminal";
}

// ── details ────────────────────────────────────────────────
function stopPoll() { clearTimeout(pollTimer); pollTimer = 0; }

async function openDetail(kind: Detail["kind"], c: Container) {
  stopPoll();
  detail.value = { kind, c };
  if (kind === "logs") {
    logText.value = "";
    await fetchLogs(true);
  } else if (kind === "inspect") {
    inspectRaw.value = "";
    try { inspectRaw.value = await d().run(`inspect ${d().q(c.ID)}`); } catch (e) { inspectRaw.value = String(e); }
  } else {
    await fetchStats();
  }
}

async function fetchLogs(initial = false) {
  const det = detail.value;
  if (!det || det.kind !== "logs") return;
  const now = Math.floor(Date.now() / 1000);
  const args = initial ? `logs --tail ${logLines.value} ${d().q(det.c.ID)}` : `logs --since ${logSince} ${d().q(det.c.ID)}`;
  try {
    const out = await d().run(args);
    logSince = now;
    if (initial) logText.value = out;
    else if (out.trim()) logText.value += (logText.value ? "\n" : "") + out;
    const lines = logText.value.split("\n");
    if (lines.length > 5000) logText.value = lines.slice(-5000).join("\n");
    await nextTick();
    if (logBox.value && !logSearch.value) logBox.value.scrollTop = logBox.value.scrollHeight;
  } catch (e) {
    if (initial) logText.value = String((e as Error)?.message ?? e);
  }
  if (logFollow.value && detail.value === det) pollTimer = window.setTimeout(() => fetchLogs(), 2000);
}

async function fetchStats() {
  const det = detail.value;
  if (!det || det.kind !== "stats") return;
  try {
    const x = d();
    stats.value = x.jsonLines<Stats>(await x.run(`stats --no-stream --format '{{json .}}' ${x.q(det.c.ID)}`))[0] ?? null;
  } catch { stats.value = null; }
  if (detail.value === det) pollTimer = window.setTimeout(fetchStats, 3000);
}

const filteredLog = computed(() => {
  const s = logSearch.value.trim().toLowerCase();
  return s ? logText.value.split("\n").filter((l) => l.toLowerCase().includes(s)).join("\n") : logText.value;
});

const inspect = computed(() => {
  try {
    const j = JSON.parse(inspectRaw.value)[0];
    return {
      id: j.Id?.slice(0, 12), image: j.Config?.Image, created: j.Created?.slice(0, 19).replace("T", " "),
      status: j.State?.Status, restart: j.HostConfig?.RestartPolicy?.Name, cmd: [].concat(j.Path ?? [], j.Args ?? []).join(" "),
      ip: Object.values(j.NetworkSettings?.Networks ?? {}).map((n: any) => n.IPAddress).filter(Boolean).join(", "),
      ports: Object.entries(j.NetworkSettings?.Ports ?? {}).map(([k, v]: [string, any]) => v ? `${v.map((b: any) => `${b.HostIp || "0.0.0.0"}:${b.HostPort}`).join(", ")} → ${k}` : k),
      mounts: (j.Mounts ?? []).map((m: any) => `${m.Source} → ${m.Destination}${m.RW ? "" : " (ro)"}`),
      env: j.Config?.Env ?? [],
    };
  } catch { return null; }
});

function closeDetail() { stopPoll(); detail.value = null; }

watch(logFollow, (f) => { stopPoll(); if (f) fetchLogs(); });

// ── images / volumes / networks / compose ─────────────────
async function pullImage() {
  const r = await prompt({ title: "Image pull", placeholder: "nginx:latest" });
  if (r?.value.trim()) doAct("pull", `pull ${d().q(r.value.trim())}`, "Pull tamamlandı");
}
async function rmImage(i: Image) {
  if (await confirm({ title: `${i.Repository}:${i.Tag} silinsin mi?`, danger: true, okText: "Sil" })) doAct(i.ID, `rmi ${d().q(i.ID)}`);
}
async function rmVolume(v: Volume) {
  if (await confirm({ title: `${v.Name} silinsin mi?`, message: "Volume içindeki veriler silinir.", danger: true, okText: "Sil" })) doAct(v.Name, `volume rm ${d().q(v.Name)}`);
}
async function rmNetwork(n: Network) {
  if (await confirm({ title: `${n.Name} silinsin mi?`, danger: true, okText: "Sil" })) doAct(n.ID, `network rm ${d().q(n.ID)}`);
}
function composeArgs(c: Compose) {
  const files = (c.ConfigFiles || "").split(",").filter(Boolean).map((f) => `-f ${d().q(f)}`).join(" ");
  return `compose -p ${d().q(c.Name)} ${files}`;
}
async function composeDo(c: Compose, verb: "up -d" | "down" | "restart") {
  if (verb === "down" && !(await confirm({ title: `${c.Name} down?`, message: "Servisler durdurulup kaldırılır.", danger: true, okText: "Down" }))) return;
  doAct(c.Name, `${composeArgs(c)} ${verb}`);
}
function composeLogs(c: Compose) {
  // Live, scrollable output belongs in the terminal.
  sh(tab.value!).type(`docker ${composeArgs(c)} logs -f --tail 200`);
  ui.page = "terminal";
}
async function prune(what: "container" | "image" | "volume" | "network" | "system") {
  const ok = await confirm({
    title: `${what} prune`, danger: true, okText: "Prune",
    message: { container: "Durmuş tüm container'lar silinir.", image: "Kullanılmayan (dangling) image'lar silinir.", volume: "Kullanılmayan volume'ler silinir — veriler kaybolur.", network: "Kullanılmayan network'ler silinir.", system: "Durmuş container'lar, kullanılmayan network'ler, dangling image'lar ve build cache silinir." }[what],
  });
  if (ok) doAct("prune", `${what} prune -f`, "Temizlendi");
}

function portsShort(p: string) {
  return [...new Set(p.split(",").map((x) => x.trim()).filter((x) => x.includes("->")).map((x) => x.replace(/^(0\.0\.0\.0|\[::\]|:::):/, "").replace("/tcp", "")))].join("  ");
}

watch(section, () => { closeDetail(); load(); });
watch(() => activeTab.value?.id, () => { tab.value = activeTab.value; closeDetail(); load(); });
onMounted(load);
onBeforeUnmount(stopPoll);
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <div class="seg">
        <button v-for="s in sections" :key="s.id" class="ib" :class="{ on: section === s.id }" :data-tip="s.tip" data-tip-pos="bottom" @click="section = s.id">
          <Icon :name="s.icon" />
        </button>
      </div>
      <div class="searchbox"><Icon name="search" :size="13" /><input v-model="filter" placeholder="filtre" spellcheck="false" /></div>
      <button v-if="section === 'images'" class="ib" data-tip="Pull" data-tip-pos="bottom" @click="pullImage"><Icon name="pull" /></button>
      <button
        v-if="section !== 'compose'" class="ib" data-tip="Prune" data-tip-pos="bottom"
        @click="prune(section === 'containers' ? 'container' : section === 'images' ? 'image' : section === 'volumes' ? 'volume' : 'network')"
      ><Icon name="broom" /></button>
      <button class="ib" data-tip="System prune" data-tip-pos="bottom" @click="prune('system')"><Icon name="zap" /></button>
      <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
    </div>

    <div v-if="error" class="msg">
      <Icon name="alert" :size="22" />
      <div>{{ error.kind === 'missing' ? 'Bu sistemde Docker yok' : error.kind === 'daemon' ? 'Docker daemon çalışmıyor' : 'Docker hatası' }}</div>
      <pre v-if="error.kind !== 'missing'" class="muted small">{{ error.msg }}</pre>
    </div>

    <div v-else class="body">
      <div class="list">
        <!-- containers -->
        <template v-if="section === 'containers'">
          <div v-for="c in shownContainers" :key="c.ID" class="row" :class="{ active: detail?.c.ID === c.ID, busy: busy.has(c.ID) }">
            <span class="dot" :class="c.State" :data-tip="c.State" />
            <div class="main">
              <div class="title">{{ c.Names }}</div>
              <div class="sub">{{ c.Image }}<span v-if="portsShort(c.Ports)"> · {{ portsShort(c.Ports) }}</span></div>
            </div>
            <span class="status">{{ c.Status }}</span>
            <div class="acts">
              <template v-if="c.State === 'running'">
                <button class="ib" data-tip="Shell (exec)" @click="cExec(c)"><Icon name="terminal" /></button>
                <button class="ib" data-tip="Durdur" @click="cAct(c, 'stop')"><Icon name="stop" /></button>
                <button class="ib" data-tip="Yeniden başlat" @click="cAct(c, 'restart')"><Icon name="refresh" /></button>
                <button class="ib" data-tip="Duraklat" @click="cAct(c, 'pause')"><Icon name="pause" /></button>
                <button class="ib danger" data-tip="Kill" @click="cKill(c)"><Icon name="zap" /></button>
              </template>
              <button v-else-if="c.State === 'paused'" class="ib" data-tip="Devam et" @click="cAct(c, 'unpause')"><Icon name="play" /></button>
              <button v-else class="ib" data-tip="Başlat" @click="cAct(c, 'start')"><Icon name="play" /></button>
              <button class="ib" data-tip="Loglar" @click="openDetail('logs', c)"><Icon name="scroll" /></button>
              <button v-if="c.State === 'running'" class="ib" data-tip="İstatistik" @click="openDetail('stats', c)"><Icon name="activity" /></button>
              <button class="ib" data-tip="Inspect" @click="openDetail('inspect', c)"><Icon name="info" /></button>
              <button class="ib danger" data-tip="Sil" @click="cRemove(c)"><Icon name="trash" /></button>
            </div>
          </div>
          <div v-if="!loading && !shownContainers.length" class="msg muted">Container yok</div>
        </template>

        <template v-else-if="section === 'images'">
          <div v-for="i in images.filter((x) => match(x.Repository, x.Tag, x.ID))" :key="i.ID + i.Tag" class="row" :class="{ busy: busy.has(i.ID) }">
            <Icon name="layers" :size="14" class="muted" />
            <div class="main"><div class="title">{{ i.Repository }}<span class="muted">:{{ i.Tag }}</span></div><div class="sub">{{ i.ID }} · {{ i.CreatedSince }}</div></div>
            <span class="status">{{ i.Size }}</span>
            <div class="acts"><button class="ib danger" data-tip="Sil" @click="rmImage(i)"><Icon name="trash" /></button></div>
          </div>
        </template>

        <template v-else-if="section === 'volumes'">
          <div v-for="v in volumes.filter((x) => match(x.Name))" :key="v.Name" class="row" :class="{ busy: busy.has(v.Name) }">
            <Icon name="database" :size="14" class="muted" />
            <div class="main"><div class="title">{{ v.Name }}</div><div class="sub">{{ v.Driver }}</div></div>
            <div class="acts"><button class="ib danger" data-tip="Sil" @click="rmVolume(v)"><Icon name="trash" /></button></div>
          </div>
        </template>

        <template v-else-if="section === 'networks'">
          <div v-for="n in networks.filter((x) => match(x.Name, x.ID))" :key="n.ID" class="row" :class="{ busy: busy.has(n.ID) }">
            <Icon name="network" :size="14" class="muted" />
            <div class="main"><div class="title">{{ n.Name }}</div><div class="sub">{{ n.Driver }} · {{ n.Scope }} · {{ n.ID }}</div></div>
            <div class="acts">
              <button v-if="!['bridge', 'host', 'none'].includes(n.Name)" class="ib danger" data-tip="Sil" @click="rmNetwork(n)"><Icon name="trash" /></button>
            </div>
          </div>
        </template>

        <template v-else>
          <div v-for="c in composes.filter((x) => match(x.Name))" :key="c.Name" class="row" :class="{ busy: busy.has(c.Name) }">
            <span class="dot" :class="c.Status.startsWith('running') ? 'running' : 'exited'" />
            <div class="main"><div class="title">{{ c.Name }}</div><div class="sub">{{ c.ConfigFiles }}</div></div>
            <span class="status">{{ c.Status }}</span>
            <div class="acts">
              <button class="ib" data-tip="Up" @click="composeDo(c, 'up -d')"><Icon name="play" /></button>
              <button class="ib" data-tip="Restart" @click="composeDo(c, 'restart')"><Icon name="refresh" /></button>
              <button class="ib" data-tip="Loglar (terminalde)" @click="composeLogs(c)"><Icon name="scroll" /></button>
              <button class="ib danger" data-tip="Down" @click="composeDo(c, 'down')"><Icon name="stop" /></button>
            </div>
          </div>
          <div v-if="!loading && !composes.length" class="msg muted">Compose projesi yok</div>
        </template>
      </div>

      <!-- detail drawer -->
      <aside v-if="detail" class="detail">
        <div class="dhead">
          <Icon :name="detail.kind === 'logs' ? 'scroll' : detail.kind === 'stats' ? 'activity' : 'info'" />
          <span class="title">{{ detail.c.Names }}</span>
          <template v-if="detail.kind === 'logs'">
            <input v-model="logSearch" class="mini" placeholder="ara" spellcheck="false" />
            <select v-model.number="logLines" class="mini" data-tip="Satır" @change="openDetail('logs', detail.c)">
              <option :value="100">100</option><option :value="200">200</option><option :value="1000">1000</option><option :value="5000">5000</option>
            </select>
            <button class="ib" :class="{ on: logFollow }" :data-tip="logFollow ? 'Canlı takip açık' : 'Canlı takip kapalı'" data-tip-pos="bottom" @click="logFollow = !logFollow"><Icon :name="logFollow ? 'pause' : 'play'" /></button>
          </template>
          <template v-if="detail.kind === 'inspect'">
            <button class="ib" :class="{ on: inspectView === 'summary' }" data-tip="Özet" data-tip-pos="bottom" @click="inspectView = 'summary'"><Icon name="list" /></button>
            <button class="ib" :class="{ on: inspectView === 'raw' }" data-tip="Ham JSON" data-tip-pos="bottom" @click="inspectView = 'raw'"><Icon name="file" /></button>
          </template>
          <button class="ib" data-tip="Kapat" data-tip-pos="bottom" @click="closeDetail"><Icon name="x" /></button>
        </div>
        <pre v-if="detail.kind === 'logs'" ref="logBox" class="log">{{ filteredLog }}</pre>
        <div v-else-if="detail.kind === 'stats'" class="stats">
          <template v-if="stats">
            <div class="stat"><span>CPU</span><b>{{ stats.CPUPerc }}</b><i :style="{ width: Math.min(100, parseFloat(stats.CPUPerc)) + '%' }" /></div>
            <div class="stat"><span>RAM</span><b>{{ stats.MemUsage }}</b><i :style="{ width: Math.min(100, parseFloat(stats.MemPerc)) + '%' }" /></div>
            <div class="stat"><span>Network</span><b>{{ stats.NetIO }}</b></div>
            <div class="stat"><span>Disk IO</span><b>{{ stats.BlockIO }}</b></div>
            <div class="stat"><span>PIDs</span><b>{{ stats.PIDs }}</b></div>
          </template>
          <div v-else class="muted">…</div>
        </div>
        <template v-else>
          <div v-if="inspectView === 'summary' && inspect" class="kv">
            <div><span>ID</span><b>{{ inspect.id }}</b></div>
            <div><span>Image</span><b>{{ inspect.image }}</b></div>
            <div><span>Durum</span><b>{{ inspect.status }}</b></div>
            <div><span>Oluşturuldu</span><b>{{ inspect.created }}</b></div>
            <div><span>Restart</span><b>{{ inspect.restart }}</b></div>
            <div><span>Komut</span><b>{{ inspect.cmd }}</b></div>
            <div v-if="inspect.ip"><span>IP</span><b>{{ inspect.ip }}</b></div>
            <div v-if="inspect.ports.length"><span>Portlar</span><b><div v-for="p in inspect.ports" :key="p">{{ p }}</div></b></div>
            <div v-if="inspect.mounts.length"><span>Mount</span><b><div v-for="m in inspect.mounts" :key="m">{{ m }}</div></b></div>
            <div v-if="inspect.env.length"><span>Env</span><b><div v-for="e in inspect.env" :key="e">{{ e }}</div></b></div>
          </div>
          <pre v-else class="log">{{ inspectRaw }}</pre>
        </template>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 2px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
.seg { display: flex; gap: 1px; margin-right: 8px; }
.ib.on { color: var(--fg); background: rgb(var(--fg-rgb) / 0.08); }
.searchbox { flex: 1; display: flex; align-items: center; gap: 6px; color: var(--muted); }
.searchbox input { flex: 1; max-width: 260px; border: 0; background: transparent; padding: 4px 0; }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.body { flex: 1; display: flex; min-height: 0; }
.list { flex: 1; overflow: auto; padding: 4px 6px 20px; min-width: 0; }
.row { display: flex; align-items: center; gap: 10px; padding: 6px 8px; border-radius: 6px; }
.row:hover, .row.active { background: rgb(var(--fg-rgb) / 0.04); }
.row.busy { opacity: .5; pointer-events: none; }
.main { flex: 1; min-width: 0; }
.title { font-family: var(--font-mono); font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sub { font-size: 11px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
.status { font-size: 11px; color: var(--muted); white-space: nowrap; }
.acts { display: flex; gap: 0; opacity: 0; transition: opacity .1s; }
.row:hover .acts, .row.active .acts { opacity: 1; }
.dot { width: 7px; height: 7px; border-radius: 50%; flex: none; background: var(--muted); }
.dot.running { background: var(--ok); }
.dot.paused, .dot.restarting { background: var(--warn); }
.dot.exited, .dot.dead { background: var(--danger); opacity: .8; }
.msg { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 50px 20px; text-align: center; }
.small { font-size: 11px; white-space: pre-wrap; max-width: 520px; }
.detail { width: min(48%, 560px); flex: none; border-left: 1px solid var(--line); display: flex; flex-direction: column; min-height: 0; }
.dhead { display: flex; align-items: center; gap: 4px; padding: 6px 8px; border-bottom: 1px solid var(--line); color: var(--muted); }
.dhead .title { flex: 1; color: var(--fg); margin-left: 4px; }
.mini { padding: 2px 6px; font-size: 11.5px; width: 90px; }
select.mini { width: auto; }
.log { flex: 1; margin: 0; overflow: auto; padding: 8px 10px; font: 11.5px/1.45 var(--font-mono); white-space: pre-wrap; word-break: break-all; user-select: text; -webkit-user-select: text; }
.stats { padding: 14px; display: flex; flex-direction: column; gap: 14px; }
.stat { display: grid; grid-template-columns: 80px 1fr; gap: 4px 10px; font-size: 12px; }
.stat span { color: var(--muted); }
.stat b { font-weight: 400; font-family: var(--font-mono); }
.stat i { grid-column: 2; height: 3px; border-radius: 2px; background: var(--accent); transition: width .3s; }
.kv { overflow: auto; padding: 10px 12px; font-size: 12px; display: flex; flex-direction: column; gap: 6px; user-select: text; -webkit-user-select: text; }
.kv > div { display: grid; grid-template-columns: 90px 1fr; gap: 10px; }
.kv span { color: var(--muted); }
.kv b { font-weight: 400; font-family: var(--font-mono); word-break: break-all; }
</style>
