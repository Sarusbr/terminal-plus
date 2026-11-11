<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { FsEntry } from "../../lib/api";
import { sh, q, isPermissionError } from "../../lib/shell";
import { activeTab, type Tab } from "../../stores/tabs";
import { confirm, prompt, toast } from "../../stores/ui";
import { openEditor } from "../../stores/editor";
import Icon from "../../components/Icon.vue";
import { listCwd, fmtSize, fmtMode, fmtDate } from "./listing";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { downloadDir } from "@tauri-apps/api/path";
import { filesCwd, startTransfer, transfers } from "../../stores/transfers";
import { api } from "../../lib/api";
import { isWinLocal, cwdOf, winCd, winCrumbs, winHome, winJoin, winParent } from "../../lib/winlocal";

const tab = ref<Tab | undefined>(activeTab.value);
const cwd = ref("");
const entries = ref<FsEntry[]>([]);
const loading = ref(false);
const error = ref("");
const selected = ref<Set<string>>(new Set());
const showHidden = ref(localStorage.getItem("tp.files.hidden") === "1");
const clip = ref<{ op: "cp" | "mv"; paths: string[]; tabId: string } | null>(null);

watch(showHidden, (v) => { try { localStorage.setItem("tp.files.hidden", v ? "1" : "0"); } catch { /* ignore */ } });

/** Windows local tab: fs via Rust, cwd from the prompt, no shell commands. */
const win = computed(() => isWinLocal(tab.value));

const isDir = (e: FsEntry) => e.kind === "d" || (e.kind === "l" && !!e.linkDir);
const rows = computed(() =>
  entries.value
    .filter((e) => showHidden.value || !e.name.startsWith("."))
    .sort((a, b) => Number(isDir(b)) - Number(isDir(a)) || a.name.localeCompare(b.name, undefined, { numeric: true })),
);
const crumbs = computed(() => {
  if (win.value) return winCrumbs(cwd.value);
  const parts = cwd.value.split("/").filter(Boolean);
  return [{ name: "/", path: "/" }, ...parts.map((p, i) => ({ name: p, path: "/" + parts.slice(0, i + 1).join("/") }))];
});
const abs = (name: string) => (win.value ? winJoin(cwd.value, name) : (cwd.value === "/" ? "/" : cwd.value + "/") + name);
const isRoot = computed(() => (win.value ? winParent(cwd.value) === cwd.value : cwd.value === "/"));
const sep = computed(() => (win.value ? "\\" : "/"));
const selPaths = () => [...selected.value].map(abs);

async function load() {
  const t = tab.value;
  if (!t) return;
  loading.value = true;
  error.value = "";
  try {
    const r = win.value
      ? await (async () => {
          const dir = cwdOf(t) || (await winHome());
          return { cwd: dir, entries: await api.localListDir(dir) };
        })()
      : await listCwd(sh(t));
    cwd.value = r.cwd;
    filesCwd[t.id] = r.cwd;
    entries.value = r.entries;
    selected.value = new Set([...selected.value].filter((n) => r.entries.some((e) => e.name === n)));
  } catch (e) {
    error.value = String((e as Error)?.message ?? e);
  } finally {
    loading.value = false;
  }
}

/** cd in the terminal itself, so terminal and panel stay in sync. */
async function cd(path: string) {
  if (!tab.value) return;
  try {
    if (win.value) await winCd(tab.value, path === ".." ? winParent(cwd.value) : path);
    else await sh(tab.value).ok(path === "~" ? "cd" : `cd -- ${q(path)}`);
    selected.value = new Set();
    await load();
  } catch (e) {
    toast(String((e as Error)?.message ?? e));
  }
}

function open(e: FsEntry) {
  if (isDir(e)) cd(abs(e.name));
  else if (tab.value) openEditor(tab.value, abs(e.name));
}

function select(e: FsEntry, ev: MouseEvent) {
  const s = new Set(ev.metaKey || ev.ctrlKey ? selected.value : []);
  if (ev.shiftKey && selected.value.size) {
    const names = rows.value.map((r) => r.name);
    const last = names.indexOf([...selected.value].pop()!);
    const cur = names.indexOf(e.name);
    names.slice(Math.min(last, cur), Math.max(last, cur) + 1).forEach((n) => s.add(n));
  } else if (s.has(e.name)) s.delete(e.name);
  else s.add(e.name);
  selected.value = s;
}

/** Run a file operation; on permission errors offer to retry with sudo. */
async function act(build: (sudo: string) => string, okMsg?: string) {
  const t = tab.value;
  if (!t) return;
  try {
    const r = await sh(t).run(build(""), 60000);
    if (r.code !== 0) {
      if (isPermissionError(r.out) && await confirm({ title: "Yetki yok", message: "sudo ile tekrar denensin mi?", okText: "sudo" })) {
        await sh(t).ok(build("sudo -p '[sudo] password for %u: ' "), 60000);
      } else throw new Error(r.out.trim().split("\n").pop() || `exit ${r.code}`);
    }
    if (okMsg) toast(okMsg);
  } catch (e) {
    toast(String((e as Error)?.message ?? e));
  }
  await load();
}

/** Windows: Rust fs operation, then reload. */
async function fsAct(fn: () => Promise<void>) {
  try { await fn(); } catch (e) { toast(String((e as Error)?.message ?? e)); }
  await load();
}

async function newFile() {
  const r = await prompt({ title: "Yeni dosya", placeholder: "dosya.txt" });
  if (!r?.value.trim()) return;
  if (win.value) return fsAct(() => api.fsOp("touch", [abs(r.value.trim())]));
  await act((s) => `${s}touch -- ${q(abs(r.value.trim()))}`);
}
async function newDir() {
  const r = await prompt({ title: "Yeni klasör", placeholder: "klasör" });
  if (!r?.value.trim()) return;
  if (win.value) return fsAct(() => api.fsOp("mkdir", [abs(r.value.trim())]));
  await act((s) => `${s}mkdir -p -- ${q(abs(r.value.trim()))}`);
}
async function rename() {
  const [name] = selected.value;
  const r = await prompt({ title: "Yeniden adlandır", value: name });
  const to = r?.value.trim();
  if (to && to !== name && win.value) return fsAct(() => api.fsOp("rename", [abs(name)], /^[A-Za-z]:[\\/]/.test(to) ? to : abs(to)));
  if (to && to !== name) await act((s) => `${s}mv -n -- ${q(abs(name))} ${q(to.startsWith("/") ? to : abs(to))}`);
}
async function remove() {
  const paths = selPaths();
  const ok = await confirm({
    title: paths.length === 1 ? `${[...selected.value][0]} silinsin mi?` : `${paths.length} öğe silinsin mi?`,
    message: "Bu işlem geri alınamaz.", danger: true, okText: "Sil",
  });
  if (ok && win.value) return fsAct(() => api.fsOp("remove", paths));
  if (ok) await act((s) => `${s}rm -rf -- ${paths.map(q).join(" ")}`);
}
async function chmod() {
  const first = entries.value.find((e) => selected.value.has(e.name));
  const r = await prompt({ title: "İzinler (chmod)", value: first ? (first.mode & 0o7777).toString(8).padStart(3, "0") : "644", placeholder: "755 / u+x" });
  const mode = r?.value.trim();
  if (!mode) return;
  if (!/^[0-7]{3,4}$/.test(mode) && !/^[ugoa]*[-+=][rwxXst]*(,[ugoa]*[-+=][rwxXst]*)*$/.test(mode)) return toast("Geçersiz mod");
  await act((s) => `${s}chmod ${mode} -- ${selPaths().map(q).join(" ")}`);
}
function copySel(op: "cp" | "mv") {
  if (!tab.value) return;
  clip.value = { op, paths: selPaths(), tabId: tab.value.id };
  toast(`${clip.value.paths.length} öğe ${op === "cp" ? "kopyalandı" : "kesildi"}`);
}
async function pasteHere() {
  const c = clip.value;
  if (!c || c.tabId !== tab.value?.id) return;
  if (win.value) {
    await fsAct(() => api.fsOp(c.op === "cp" ? "copy" : "move", c.paths, cwd.value));
    if (c.op === "mv") clip.value = null;
    return;
  }
  const srcs = c.paths.map(q).join(" ");
  await act((s) => (c.op === "cp" ? `${s}cp -R -- ${srcs} ${q(cwd.value)}` : `${s}mv -- ${srcs} ${q(cwd.value)}`));
  if (c.op === "mv") clip.value = null;
}
async function download() {
  const t = tab.value;
  if (!t) return;
  const dir = await openDialog({ directory: true, defaultPath: await downloadDir().catch(() => undefined), title: "İndirme klasörü" });
  if (typeof dir === "string") startTransfer(t, "down", selPaths(), dir);
}

async function copyPath() {
  const paths = selected.value.size ? selPaths() : [cwd.value];
  await writeText(paths.join("\n"));
  toast("Yol kopyalandı");
}

watch(() => activeTab.value?.id, () => {
  tab.value = activeTab.value;
  selected.value = new Set();
  load();
});
watch(() => transfers.version, load);
// Windows: follow `cd` typed in the terminal while the page is open.
watch(() => (win.value && tab.value ? cwdOf(tab.value) : undefined), (c) => { if (c && c !== cwd.value) load(); });
onMounted(load);
</script>

<template>
  <div class="page" @keydown.delete.self="selected.size && remove()">
    <div class="toolbar">
      <button class="ib" data-tip="Üst dizin" data-tip-pos="bottom" :disabled="isRoot" @click="cd('..')"><Icon name="arrowUp" /></button>
      <button class="ib" data-tip="Ev dizini" data-tip-pos="bottom" @click="cd('~')"><Icon name="home" /></button>
      <nav class="crumbs">
        <template v-for="(c, i) in crumbs" :key="c.path">
          <button class="crumb" :class="{ last: i === crumbs.length - 1 }" @click="cd(c.path)">{{ c.name }}</button>
          <span v-if="i > 0 && i < crumbs.length - 1" class="sep">{{ sep }}</span>
        </template>
      </nav>
      <div class="tools">
        <template v-if="selected.size">
          <span class="count">{{ selected.size }}</span>
          <button class="ib" data-tip="Yeniden adlandır" data-tip-pos="bottom" :disabled="selected.size !== 1" @click="rename"><Icon name="edit" /></button>
          <button class="ib" data-tip="Kopyala" data-tip-pos="bottom" @click="copySel('cp')"><Icon name="copy" /></button>
          <button class="ib" data-tip="Kes (taşı)" data-tip-pos="bottom" @click="copySel('mv')"><Icon name="cut" /></button>
          <button v-if="!win" class="ib" data-tip="İzinler" data-tip-pos="bottom" @click="chmod"><Icon name="chmod" /></button>
          <button class="ib" data-tip="Yolu kopyala" data-tip-pos="bottom" @click="copyPath"><Icon name="link" /></button>
          <button class="ib" data-tip="İndir" data-tip-pos="bottom" @click="download"><Icon name="pull" /></button>
          <button class="ib danger" data-tip="Sil" data-tip-pos="bottom" @click="remove"><Icon name="trash" /></button>
          <span class="div" />
        </template>
        <button v-if="clip && clip.tabId === tab?.id" class="ib" :data-tip="`Buraya yapıştır (${clip.paths.length})`" data-tip-pos="bottom" @click="pasteHere"><Icon name="clipboard" /></button>
        <button class="ib" data-tip="Yeni dosya" data-tip-pos="bottom" @click="newFile"><Icon name="filePlus" /></button>
        <button class="ib" data-tip="Yeni klasör" data-tip-pos="bottom" @click="newDir"><Icon name="folderPlus" /></button>
        <button class="ib" :data-tip="showHidden ? 'Gizlileri gizle' : 'Gizlileri göster'" data-tip-pos="bottom" @click="showHidden = !showHidden">
          <Icon :name="showHidden ? 'eye' : 'eyeOff'" />
        </button>
        <button class="ib" data-tip="Yenile" data-tip-pos="bottom" :class="{ spin: loading }" @click="load"><Icon name="refresh" /></button>
      </div>
    </div>

    <div v-if="error" class="msg err"><Icon name="alert" /> {{ error }}</div>
    <div v-else class="list" tabindex="0" @click.self="selected = new Set()">
      <div
        v-for="e in rows" :key="e.name" class="row" :class="{ sel: selected.has(e.name), hidden: e.name.startsWith('.') }"
        @click="select(e, $event)" @dblclick="open(e)"
      >
        <Icon :name="isDir(e) ? 'folder' : e.kind === 'l' ? 'fileLink' : 'file'" :size="15" :class="isDir(e) ? 'dir' : 'muted'" />
        <span class="name" @click.stop="open(e)">{{ e.name }}<span v-if="e.link" class="link"> → {{ e.link }}</span></span>
        <span class="size">{{ isDir(e) ? '' : fmtSize(e.size) }}</span>
        <span class="mode">{{ fmtMode(e.kind, e.mode) }}</span>
        <span class="date">{{ fmtDate(e.mtime) }}</span>
      </div>
      <div v-if="!loading && !rows.length" class="msg muted">Boş klasör</div>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 2px; padding: 6px 10px; border-bottom: 1px solid var(--line); min-height: 42px; }
.crumbs { flex: 1; min-width: 0; display: flex; align-items: center; overflow: hidden; font-family: var(--font-mono); font-size: 12.5px; margin-left: 6px; }
.crumb { padding: 3px 4px; border-radius: 4px; color: var(--muted); white-space: nowrap; }
.crumb:hover { color: var(--fg); background: rgb(var(--fg-rgb) / 0.06); }
.crumb.last { color: var(--fg); }
.sep { color: var(--muted); opacity: .5; }
.tools { display: flex; align-items: center; gap: 1px; }
.count { font-size: 11px; color: var(--accent); margin-right: 4px; font-family: var(--font-mono); }
.div { width: 1px; height: 16px; background: var(--line); margin: 0 4px; }
.spin :deep(svg) { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.list { flex: 1; overflow: auto; padding: 4px 6px 20px; outline: none; font-family: var(--font-mono); font-size: 12.5px; }
.row { display: grid; grid-template-columns: 18px minmax(0, 1fr) 70px 90px 130px; align-items: center; gap: 10px; padding: 4px 8px; border-radius: 5px; }
.row:hover { background: rgb(var(--fg-rgb) / 0.04); }
.row.sel { background: rgb(var(--fg-rgb) / 0.09); }
.row.hidden .name { opacity: .6; }
.name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
.name:hover { text-decoration: underline; text-decoration-color: var(--muted); }
.link { color: var(--muted); }
.dir { color: var(--accent); }
.size, .mode, .date { color: var(--muted); font-size: 11.5px; white-space: nowrap; }
.size { text-align: right; }
.msg { display: flex; gap: 8px; align-items: center; justify-content: center; padding: 40px; }
.err { color: var(--danger); }
@media (max-width: 640px) { .row { grid-template-columns: 18px minmax(0, 1fr) 70px; } .mode, .date { display: none; } }
</style>
