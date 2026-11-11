<script setup lang="ts">
import { computed, ref } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api } from "../lib/api";
import { servers, newServer, upsertServer, removeServer, importSshConfig, type Server } from "../stores/servers";
import { openServer } from "../stores/tabs";
import { confirm, toast } from "../stores/ui";
import Icon from "../components/Icon.vue";

const q = ref("");
const editing = ref<Server | null>(null);
const secret = ref("");
const hasSecret = ref(false);
const tagsText = ref("");

const groups = computed(() => {
  const needle = q.value.trim().toLowerCase();
  const list = servers.list.filter((s) =>
    !needle || [s.name, s.host, s.user, s.group ?? "", ...s.tags].some((v) => v.toLowerCase().includes(needle)));
  const map = new Map<string, Server[]>();
  for (const s of list.sort((a, b) => (a.name || a.host).localeCompare(b.name || b.host))) {
    const g = s.group?.trim() || "";
    if (!map.has(g)) map.set(g, []);
    map.get(g)!.push(s);
  }
  return [...map.entries()].sort(([a], [b]) => (a === "" ? -1 : b === "" ? 1 : a.localeCompare(b)));
});

async function edit(s?: Server) {
  editing.value = s ? { ...s, tags: [...s.tags] } : newServer();
  tagsText.value = editing.value.tags.join(", ");
  secret.value = "";
  hasSecret.value = s ? await api.secretExists(s.auth === "password" ? "pw" : "pp", s.id).catch(() => false) : false;
}

async function save() {
  const s = editing.value;
  if (!s || !s.host.trim()) return;
  s.host = s.host.trim();
  s.name = s.name.trim() || s.host;
  s.port = Number(s.port) || 22;
  s.tags = tagsText.value.split(",").map((t) => t.trim()).filter(Boolean);
  if (secret.value) await api.secretSet(s.auth === "password" ? "pw" : "pp", s.id, secret.value);
  await upsertServer(s);
  editing.value = null;
}

async function forgetSecret() {
  const s = editing.value;
  if (!s) return;
  await api.secretDelete(s.auth === "password" ? "pw" : "pp", s.id);
  hasSecret.value = false;
}

async function browseKey() {
  const p = await openDialog({ multiple: false, directory: false, title: "SSH key" });
  if (typeof p === "string" && editing.value) editing.value.keyPath = p;
}

async function del(s: Server) {
  if (await confirm({ title: `${s.name} silinsin mi?`, message: "Kayıtlı şifre/passphrase de keychain'den silinir.", danger: true, okText: "Sil" }))
    await removeServer(s.id);
}

async function doImport() {
  try {
    const n = await importSshConfig();
    toast(n ? `${n} sunucu içe aktarıldı` : "Yeni sunucu bulunamadı");
  } catch (e) {
    toast(String(e));
  }
}
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <div class="searchbox">
        <Icon name="search" :size="14" />
        <input v-model="q" placeholder="ara" spellcheck="false" />
      </div>
      <button class="ib" data-tip="~/.ssh/config içe aktar" @click="doImport"><Icon name="import" /></button>
      <button class="ib" data-tip="Sunucu ekle" @click="edit()"><Icon name="plus" /></button>
    </div>

    <form v-if="editing" class="editor" @submit.prevent="save" @keydown.esc="editing = null">
      <div class="grid">
        <label>Ad<input v-model="editing.name" placeholder="prod-api" /></label>
        <label>Grup<input v-model="editing.group" placeholder="—" /></label>
        <label class="wide">Host<input v-model="editing.host" placeholder="10.0.0.5 / example.com" required autofocus /></label>
        <label class="port">Port<input v-model.number="editing.port" type="number" min="1" max="65535" /></label>
        <label>Kullanıcı<input v-model="editing.user" /></label>
        <label>Etiketler<input v-model="tagsText" placeholder="api, prod" /></label>
      </div>
      <div class="auth">
        <div class="seg">
          <button type="button" :class="{ on: editing.auth === 'key' }" @click="editing.auth = 'key'; hasSecret = false">
            <Icon name="key" :size="14" /> SSH key
          </button>
          <button type="button" :class="{ on: editing.auth === 'password' }" @click="editing.auth = 'password'; hasSecret = false">
            <Icon name="lock" :size="14" /> Şifre
          </button>
        </div>
        <div v-if="editing.auth === 'key'" class="keyrow">
          <input v-model="editing.keyPath" placeholder="~/.ssh/id_ed25519 (boş: varsayılan)" spellcheck="false" />
          <button type="button" class="ib" data-tip="Dosya seç" @click="browseKey"><Icon name="folder" /></button>
        </div>
        <div class="keyrow">
          <input
            v-model="secret" type="password" autocomplete="off"
            :placeholder="hasSecret ? '•••••• (keychain\'de kayıtlı)' : editing.auth === 'key' ? 'passphrase (opsiyonel)' : 'şifre (opsiyonel, keychain)'"
          />
          <button v-if="hasSecret" type="button" class="ib" data-tip="Keychain'den sil" @click="forgetSecret"><Icon name="trash" /></button>
        </div>
      </div>
      <div class="actions">
        <button type="button" class="btn" @click="editing = null">Vazgeç</button>
        <button type="submit" class="btn primary"><Icon name="check" :size="14" /> Kaydet</button>
      </div>
    </form>

    <div class="list">
      <div v-if="!servers.list.length && !editing" class="empty">
        <Icon name="server" :size="28" />
        <div>Kayıtlı sunucu yok</div>
        <div class="row">
          <button class="btn primary" @click="edit()"><Icon name="plus" :size="14" /> Ekle</button>
          <button class="btn" @click="doImport"><Icon name="import" :size="14" /> ~/.ssh/config</button>
        </div>
      </div>
      <section v-for="[g, list] in groups" :key="g">
        <h4 v-if="g">{{ g }}</h4>
        <div
          v-for="s in list" :key="s.id" class="srv" tabindex="0"
          @dblclick="openServer(s)" @keydown.enter="openServer(s)"
        >
          <Icon :name="s.auth === 'key' ? 'key' : 'lock'" :size="14" class="muted" />
          <div class="info">
            <span class="name">{{ s.name }}</span>
            <span class="addr">{{ s.user }}@{{ s.host }}<template v-if="s.port !== 22">:{{ s.port }}</template></span>
          </div>
          <span v-for="t in s.tags" :key="t" class="tag">{{ t }}</span>
          <div class="acts">
            <button class="ib" data-tip="Bağlan" @click="openServer(s)"><Icon name="link" /></button>
            <button class="ib" data-tip="Düzenle" @click="edit(s)"><Icon name="edit" /></button>
            <button class="ib danger" data-tip="Sil" @click="del(s)"><Icon name="trash" /></button>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 4px; padding: 10px 14px; border-bottom: 1px solid var(--line); }
.searchbox { flex: 1; display: flex; align-items: center; gap: 8px; color: var(--muted); }
.searchbox input { flex: 1; border: 0; background: transparent; padding: 4px 0; }
.list { flex: 1; overflow: auto; padding: 6px 8px 16px; }
h4 { margin: 14px 8px 4px; font-size: 11px; font-weight: 500; color: var(--muted); text-transform: uppercase; letter-spacing: .06em; }
.srv {
  display: flex; align-items: center; gap: 10px; padding: 7px 8px; border-radius: 6px;
  cursor: default; outline: none;
}
.srv:hover, .srv:focus { background: rgb(var(--fg-rgb) / 0.05); }
.info { flex: 1; min-width: 0; display: flex; align-items: baseline; gap: 10px; }
.name { white-space: nowrap; }
.addr { font-family: var(--font-mono); font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tag { font-size: 10.5px; padding: 1px 6px; border-radius: 8px; color: var(--muted); border: 1px solid var(--line); }
.acts { display: flex; gap: 2px; opacity: 0; transition: opacity .1s; }
.srv:hover .acts, .srv:focus-within .acts { opacity: 1; }
.empty { display: flex; flex-direction: column; align-items: center; gap: 10px; padding: 60px 0; color: var(--muted); }
.row { display: flex; gap: 8px; }
.editor { margin: 12px 14px 4px; padding: 14px; border: 1px solid var(--line); border-radius: 8px; display: flex; flex-direction: column; gap: 12px; background: rgb(var(--fg-rgb) / 0.02); }
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.grid label { display: flex; flex-direction: column; gap: 4px; font-size: 11px; color: var(--muted); }
.grid .wide { grid-column: span 1; }
.auth { display: flex; flex-direction: column; gap: 8px; }
.seg { display: inline-flex; align-self: flex-start; border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
.seg button { display: flex; align-items: center; gap: 6px; padding: 5px 10px; color: var(--muted); font-size: 12px; }
.seg button.on { color: var(--fg); background: rgb(var(--fg-rgb) / 0.08); }
.keyrow { display: flex; gap: 6px; align-items: center; }
.keyrow input { flex: 1; }
.actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
