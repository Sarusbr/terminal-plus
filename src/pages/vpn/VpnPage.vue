<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import Icon from "../../components/Icon.vue";
import { confirm, toast } from "../../stores/ui";
import { fmtBytes } from "../../stores/transfers";
import {
  vpn, OC_PROTOCOLS, appModeSupported, newProfile, saveProfile, removeProfile, connect, disconnect,
  installComponent, refreshComponent, fmtDuration, type VpnProfile, type VpnMode,
} from "../../stores/vpn";

const editing = ref<VpnProfile | null>(null);
const password = ref("");
const hasSaved = ref(false);
const details = ref<string | null>(null);
const revealed = ref<string | null>(null);
const logFor = ref<string | null>(null);
const logLines = ref<string[]>([]);
const now = ref(Date.now());
let tick = 0;

const st = (id: string) => vpn.status[id];
const live = (id: string) => ["connecting", "connected", "reconnecting"].includes(st(id)?.state ?? "");
const protoName = (p: VpnProfile) => OC_PROTOCOLS.find((x) => x.id === p.ocProtocol)?.name ?? p.protocol;
const modeName = (m?: VpnMode) => (m === "app" ? "uygulama" : "genel");

async function edit(p?: VpnProfile) {
  editing.value = p ? { ...p } : newProfile();
  password.value = "";
  hasSaved.value = p ? await invoke<boolean>("vpn_secret_exists", { id: p.id }).catch(() => false) : false;
}
async function save() {
  const p = editing.value;
  if (!p || !p.server.trim()) return;
  p.server = p.server.trim();
  p.name = p.name.trim() || p.server;
  await saveProfile(p, password.value ? password.value : undefined);
  editing.value = null;
}
async function forgetPassword() {
  if (!editing.value) return;
  await invoke("vpn_secret_set", { id: editing.value.id, secret: null });
  hasSaved.value = false;
}
async function browse(field: "certPath" | "configPath") {
  const f = await openDialog({ multiple: false, directory: false });
  if (typeof f === "string" && editing.value) editing.value[field] = f;
}
async function del(p: VpnProfile) {
  if (await confirm({ title: `${p.name} silinsin mi?`, message: "Kayıtlı şifresi de keychain'den silinir.", danger: true, okText: "Sil" }))
    await removeProfile(p.id);
}
async function go(p: VpnProfile, mode: VpnMode) {
  try {
    await connect(p, mode);
  } catch (e) {
    const m = String((e as Error)?.message ?? e);
    const s = st(p.id);
    if (s?.errorKind === "auth") {
      if (await confirm({ title: "Kimlik doğrulama başarısız", message: m, okText: "Bilgileri düzenle" })) edit(p);
    } else if (s?.errorKind === "denied" && appModeSupported()) {
      if (await confirm({ title: "Yönetici izni verilmedi", message: "Genel VPN yönetici izni gerektirir. Bunun yerine yalnızca bu uygulama için bağlanılsın mı?", okText: "Uygulama için bağlan" }))
        go(p, "app");
    } else if (m !== "Sertifika reddedildi") toast(m);
  }
}
async function showDetails(p: VpnProfile) {
  details.value = details.value === p.id ? null : p.id;
  revealed.value = null;
}
async function reveal(p: VpnProfile) {
  revealed.value = revealed.value === null ? (await invoke<string | null>("vpn_secret_reveal", { id: p.id })) ?? "" : null;
}
async function copyLog(p: VpnProfile) {
  const lines = await invoke<string[]>("vpn_log", { id: p.id }).catch(() => []);
  const s = st(p.id);
  const head = [`${p.name} · ${protoName(p)} · ${p.server}`, s?.error ? `Hata: ${s.error}` : ""].filter(Boolean);
  await writeText([...head, "", ...(lines.length ? lines : logLines.value)].join("\n"));
  toast("Log panoya kopyalandı");
}
async function showLog(id: string) {
  logFor.value = logFor.value === id ? null : id;
  if (logFor.value) logLines.value = await invoke<string[]>("vpn_log", { id });
}

const sorted = computed(() => [...vpn.profiles].sort((a, b) => Number(live(b.id)) - Number(live(a.id)) || a.name.localeCompare(b.name)));

onMounted(() => {
  refreshComponent();
  tick = window.setInterval(async () => {
    now.value = Date.now();
    for (const s of await invoke<typeof vpn.status[string][]>("vpn_list").catch(() => [])) vpn.status[s.id] = { ...vpn.status[s.id], ...s };
    if (logFor.value) logLines.value = await invoke<string[]>("vpn_log", { id: logFor.value }).catch(() => []);
  }, 2000);
});
onBeforeUnmount(() => clearInterval(tick));
</script>

<template>
  <div class="page">
    <div class="toolbar">
      <Icon name="vpn" class="muted" />
      <span class="title">VPN</span>
      <span class="grow" />
      <span v-if="vpn.download" class="dl">
        <Icon name="pull" :size="13" /> OpenConnect {{ vpn.download.total ? Math.round((vpn.download.done / vpn.download.total) * 100) + '%' : '…' }}
        <i class="bar"><i :style="{ width: (vpn.download.total ? (vpn.download.done / vpn.download.total) * 100 : 5) + '%' }" /></i>
      </span>
      <button v-else-if="vpn.component === 'missing'" class="btn" @click="installComponent"><Icon name="pull" :size="14" /> OpenConnect'i indir</button>
      <span v-else-if="vpn.component !== 'unknown'" class="muted small" :data-tip="vpn.component === 'system' ? 'Sistemde kurulu OpenConnect kullanılıyor' : 'İndirilen OpenConnect bileşeni'" data-tip-pos="bottom">
        <Icon name="check" :size="12" /> OpenConnect
      </span>
      <button class="ib" data-tip="Profil ekle" data-tip-pos="bottom" @click="edit()"><Icon name="plus" /></button>
    </div>

    <form v-if="editing" class="editor" @submit.prevent="save" @keydown.esc="editing = null">
      <div class="grid">
        <label>Ad<input v-model="editing.name" placeholder="Şirket VPN" /></label>
        <label>Protokol
          <select v-model="editing.protocol">
            <option value="openconnect">OpenConnect</option>
            <option value="wireguard" disabled>WireGuard (yakında)</option>
            <option value="openvpn" disabled>OpenVPN (yakında)</option>
          </select>
        </label>
        <label v-if="editing.protocol === 'openconnect'">Sunucu türü
          <select v-model="editing.ocProtocol">
            <option v-for="p in OC_PROTOCOLS" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
        </label>
        <label>Portal / sunucu<input v-model="editing.server" placeholder="vpn.sirket.com" required spellcheck="false" /></label>
        <label>Kullanıcı<input v-model="editing.user" spellcheck="false" autocomplete="off" /></label>
        <label>Grup / gateway<input v-model="editing.group" placeholder="otomatik / sorulsun" spellcheck="false" /></label>
        <label>Şifre
          <span class="row">
            <input v-model="password" type="password" autocomplete="off" :placeholder="hasSaved ? '•••••• (keychain)' : 'keychain\'e kaydedilir'" />
            <button v-if="hasSaved" type="button" class="ib" data-tip="Kayıtlı şifreyi sil" @click="forgetPassword"><Icon name="trash" /></button>
          </span>
        </label>
        <label>Sertifika (opsiyonel)
          <span class="row">
            <input v-model="editing.certPath" placeholder="istemci sertifikası (.pem/.p12)" spellcheck="false" />
            <button type="button" class="ib" data-tip="Dosya seç" @click="browse('certPath')"><Icon name="folder" /></button>
          </span>
        </label>
        <label v-if="appModeSupported()">Varsayılan mod
          <select v-model="editing.defaultMode">
            <option value="app">Uygulama için</option>
            <option value="global">Genel</option>
          </select>
        </label>
      </div>
      <div v-if="editing.servercert" class="pin muted"><Icon name="shield" :size="12" /> Sabitlenmiş sertifika: <code>{{ editing.servercert }}</code>
        <button type="button" class="link" @click="editing.servercert = undefined">kaldır</button></div>
      <div class="actions">
        <button type="button" class="btn" @click="editing = null">Vazgeç</button>
        <button type="submit" class="btn primary"><Icon name="check" :size="14" /> Kaydet</button>
      </div>
    </form>

    <div class="list">
      <div v-if="!vpn.profiles.length && !editing" class="empty">
        <Icon name="vpn" :size="28" />
        <div>Kayıtlı VPN profili yok</div>
        <button class="btn primary" @click="edit()"><Icon name="plus" :size="14" /> Profil ekle</button>
      </div>

      <div v-for="p in sorted" :key="p.id" class="card" :class="{ on: st(p.id)?.state === 'connected' }">
        <div class="head">
          <span class="dot" :class="st(p.id)?.state ?? 'off'" :data-tip="st(p.id)?.state ?? 'kapalı'" />
          <Icon name="vpn" :size="15" class="muted" />
          <div class="main">
            <div class="name">{{ p.name }}</div>
            <div class="sub">{{ protoName(p) }} · {{ p.server }}<template v-if="p.user"> · {{ p.user }}</template></div>
          </div>

          <template v-if="live(p.id)">
            <span class="mode"><Icon :name="st(p.id)?.mode === 'app' ? 'terminal' : 'globe'" :size="12" /> {{ modeName(st(p.id)?.mode) }}</span>
            <button class="btn danger-o" @click="disconnect(p.id)"><Icon name="unplug" :size="14" /> Bağlantıyı kes</button>
          </template>
          <template v-else>
            <template v-if="appModeSupported()">
              <button class="ib big" data-tip="Uygulama İçin Bağlan — sadece Terminal Plus trafiği, yönetici izni gerekmez" @click="go(p, 'app')"><Icon name="terminal" /></button>
              <button class="ib big" data-tip="Genel Bağlan — tüm sistem, yönetici izni ister" @click="go(p, 'global')"><Icon name="globe" /></button>
            </template>
            <button v-else class="btn primary" @click="go(p, 'global')"><Icon name="link" :size="14" /> Bağlan</button>
          </template>
          <div class="acts">
            <button class="ib" data-tip="Bilgiler" @click="showDetails(p)"><Icon name="info" /></button>
            <button class="ib" data-tip="Düzenle" @click="edit(p)"><Icon name="edit" /></button>
            <button class="ib danger" data-tip="Sil" @click="del(p)"><Icon name="trash" /></button>
          </div>
        </div>

        <div v-if="st(p.id)?.state === 'error' && st(p.id)?.error" class="err">
          <Icon name="alert" :size="13" />
          <span class="errtext">{{ st(p.id)?.error }}</span>
          <button v-if="st(p.id)?.errorKind === 'auth'" class="link" @click="edit(p)">bilgileri düzenle</button>
          <button class="link" @click="showLog(p.id)">log</button>
          <button class="link" @click="copyLog(p)">logu kopyala</button>
        </div>

        <div v-if="live(p.id)" class="stats">
          <span v-if="st(p.id)?.state !== 'connected'" class="muted">{{ st(p.id)?.state === 'reconnecting' ? 'yeniden bağlanıyor…' : 'bağlanıyor…' }}</span>
          <template v-else>
            <span><Icon name="activity" :size="12" /> {{ st(p.id)?.since ? fmtDuration(now - (st(p.id)?.since ?? now)) : '' }}</span>
            <span><b>IP</b> {{ st(p.id)?.ip }}</span>
            <span><Icon name="down" :size="12" /> {{ fmtBytes(st(p.id)?.rx ?? 0) }}</span>
            <span><Icon name="up" :size="12" /> {{ fmtBytes(st(p.id)?.tx ?? 0) }}</span>
            <span v-if="st(p.id)?.socksPort" class="muted">SOCKS 127.0.0.1:{{ st(p.id)?.socksPort }}</span>
          </template>
          <span class="grow" />
          <button class="ib" :class="{ on: logFor === p.id }" data-tip="Log" @click="showLog(p.id)"><Icon name="scroll" :size="14" /></button>
        </div>
        <div v-if="logFor === p.id" class="logwrap">
          <button class="ib copy" data-tip="Logu kopyala" @click="copyLog(p)"><Icon name="copy" :size="13" /></button>
          <pre class="log">{{ logLines.join('\n') || '—' }}</pre>
        </div>

        <div v-if="details === p.id" class="kv">
          <div><span>Protokol</span><b>{{ protoName(p) }}</b></div>
          <div><span>Sunucu</span><b>{{ p.server }}</b></div>
          <div><span>Kullanıcı</span><b>{{ p.user || '—' }}</b></div>
          <div><span>Grup</span><b>{{ p.group || '—' }}</b></div>
          <div><span>Şifre</span><b>
            <code>{{ revealed ?? '••••••••' }}</code>
            <button class="ib" :data-tip="revealed === null ? 'Göster' : 'Gizle'" @click="reveal(p)"><Icon :name="revealed === null ? 'eye' : 'eyeOff'" :size="13" /></button>
          </b></div>
          <div v-if="p.certPath"><span>Sertifika</span><b>{{ p.certPath }}</b></div>
          <div v-if="p.servercert"><span>Sunucu pin</span><b>{{ p.servercert }}</b></div>
          <div><span>Varsayılan mod</span><b>{{ modeName(p.defaultMode) }}</b></div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; height: 100%; }
.toolbar { display: flex; align-items: center; gap: 8px; padding: 6px 10px 6px 14px; border-bottom: 1px solid var(--line); min-height: 42px; }
.title { font-weight: 500; }
.grow { flex: 1; }
.small { font-size: 11.5px; display: inline-flex; gap: 4px; align-items: center; }
.dl { display: inline-flex; align-items: center; gap: 6px; font-size: 11.5px; color: var(--muted); }
.dl .bar { display: inline-block; width: 90px; height: 3px; border-radius: 2px; background: rgb(var(--fg-rgb) / 0.08); overflow: hidden; }
.dl .bar i { display: block; height: 100%; background: var(--accent); transition: width .2s; }
.list { flex: 1; overflow: auto; padding: 10px 12px 20px; display: flex; flex-direction: column; gap: 8px; }
.empty { display: flex; flex-direction: column; align-items: center; gap: 10px; padding: 60px 0; color: var(--muted); }
.card { flex: none; border: 1px solid var(--line); border-radius: 8px; padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; }
.card.on { border-color: rgb(from var(--ok) r g b / 0.4); }
.head { display: flex; align-items: center; gap: 10px; }
.main { flex: 1; min-width: 0; }
.name { font-weight: 500; }
.sub { font-size: 11.5px; color: var(--muted); font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--muted); opacity: .5; flex: none; }
.dot.connected { background: var(--ok); opacity: 1; }
.dot.connecting, .dot.reconnecting { background: var(--warn); opacity: 1; animation: pulse 1s infinite; }
.dot.error { background: var(--danger); opacity: 1; }
@keyframes pulse { 50% { opacity: .35; } }
.mode { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; color: var(--muted); }
.ib.big { width: 32px; height: 32px; }
.ib.on { color: var(--fg); }
.btn.danger-o { color: var(--danger); }
.acts { display: flex; gap: 0; margin-left: 4px; opacity: .55; }
.card:hover .acts { opacity: 1; }
.err { font-size: 12px; color: var(--danger); display: flex; gap: 6px; align-items: flex-start; flex-wrap: wrap; }
.errtext { flex: 1 1 260px; min-width: 0; word-break: break-word; white-space: pre-wrap; user-select: text; -webkit-user-select: text; font-family: var(--font-mono); font-size: 11.5px; }
.logwrap { position: relative; }
.logwrap .copy { position: absolute; top: 4px; right: 4px; width: 24px; height: 24px; background: rgb(var(--bg-rgb) / 0.8); }
.link { color: var(--accent); font-size: 11.5px; margin-left: 6px; }
.stats { display: flex; align-items: center; gap: 16px; font-size: 12px; font-family: var(--font-mono); color: var(--fg); padding-left: 18px; }
.stats span { display: inline-flex; align-items: center; gap: 4px; }
.stats b { font-weight: 400; color: var(--muted); }
.log { margin: 0; max-height: 220px; overflow: auto; padding: 8px 10px; border-radius: 6px; background: rgb(var(--fg-rgb) / 0.04); font: 11px/1.45 var(--font-mono); white-space: pre-wrap; word-break: break-all; user-select: text; -webkit-user-select: text; }
.kv { display: flex; flex-direction: column; gap: 5px; font-size: 12px; padding-left: 18px; }
.kv > div { display: grid; grid-template-columns: 110px 1fr; gap: 10px; align-items: center; }
.kv span { color: var(--muted); }
.kv b { font-weight: 400; font-family: var(--font-mono); display: flex; align-items: center; gap: 6px; word-break: break-all; user-select: text; -webkit-user-select: text; }
.editor { margin: 12px 14px 4px; padding: 14px; border: 1px solid var(--line); border-radius: 8px; display: flex; flex-direction: column; gap: 12px; background: rgb(var(--fg-rgb) / 0.02); }
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.grid label { display: flex; flex-direction: column; gap: 4px; font-size: 11px; color: var(--muted); }
.row { display: flex; gap: 6px; align-items: center; }
.row input { flex: 1; min-width: 0; }
.pin { font-size: 11px; display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
.pin code { font-family: var(--font-mono); word-break: break-all; }
.actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
