<script setup lang="ts">
import { transfers, cancelTransfer, dismissTransfer, fmtBytes, fmtEta, type Transfer } from "../stores/transfers";
import Icon from "./Icon.vue";

const eta = (t: Transfer) => (t.speed > 0 ? (t.total - t.bytes) / t.speed : Infinity);
const pct = (a: number, b: number) => (b ? Math.min(100, (a / b) * 100) : 100);

/** Show a window of rows around the current file so huge trees stay light. */
function visible(t: Transfer) {
  const start = Math.max(0, Math.min(t.index - 3, t.files.length - 60));
  return t.files.slice(start, start + 60).map((f, i) => ({ f, i: start + i }));
}
function fileProgress(t: Transfer, i: number, size: number) {
  if (i < t.index || t.state === "done") return 100;
  if (i === t.index) return pct(t.fileDone, size);
  return 0;
}
</script>

<template>
  <transition-group name="slide" tag="div" class="transfers">
    <div v-for="t in transfers.list" :key="t.id" class="tr">
      <div class="head">
        <Icon :name="t.up ? 'arrowUp' : 'down'" :size="14" :class="t.state === 'error' ? 'err' : 'acc'" />
        <span class="title">
          {{ t.files.length || '…' }} dosya
          <span class="muted">· {{ t.up ? '→' : '←' }} {{ t.label }}</span>
        </span>
        <button v-if="t.state === 'planning' || t.state === 'running'" class="ib" data-tip="İptal" @click="cancelTransfer(t.id)"><Icon name="x" :size="13" /></button>
        <button v-else class="ib" data-tip="Kapat" @click="dismissTransfer(t.id)"><Icon name="x" :size="13" /></button>
      </div>
      <div class="bar"><i :class="t.state" :style="{ width: pct(t.bytes, t.total) + '%' }" /></div>
      <div class="meta">
        <span>{{ fmtBytes(t.bytes) }} / {{ fmtBytes(t.total) }}</span>
        <template v-if="t.state === 'running'">
          <span>{{ fmtBytes(t.speed) }}/s</span>
          <span>{{ fmtEta(eta(t)) }} kaldı</span>
        </template>
        <span v-else-if="t.state === 'planning'">hazırlanıyor…</span>
        <span v-else-if="t.state === 'done'" class="ok">tamamlandı</span>
        <span v-else-if="t.state === 'cancelled'">iptal edildi</span>
        <span v-else class="err" :title="t.error">{{ t.error }}</span>
      </div>
      <div v-if="t.files.length > 1 || t.state !== 'done'" class="files">
        <div v-for="{ f, i } in visible(t)" :key="i" class="file" :class="{ cur: i === t.index && t.state === 'running' }">
          <span class="fname" :title="f.name">{{ f.name }}</span>
          <span class="fsize">{{ fmtBytes(f.size) }}</span>
          <div class="fbar"><i :style="{ width: fileProgress(t, i, f.size) + '%' }" /></div>
        </div>
      </div>
    </div>
  </transition-group>
</template>

<style scoped>
.transfers { position: fixed; right: 14px; bottom: 14px; z-index: 90; display: flex; flex-direction: column; gap: 8px; width: min(340px, calc(100vw - 28px)); }
.tr { padding: 10px 12px; border-radius: 8px; border: 1px solid var(--line); background: rgb(var(--bg-rgb) / 0.97); box-shadow: 0 8px 28px rgb(0 0 0 / 0.35); font-size: 12px; }
.head { display: flex; align-items: center; gap: 8px; }
.title { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.acc { color: var(--accent); }
.err { color: var(--danger); }
.ok { color: var(--ok); }
.ib { width: 22px; height: 22px; }
.bar, .fbar { height: 3px; border-radius: 2px; background: rgb(var(--fg-rgb) / 0.08); overflow: hidden; }
.bar { margin: 8px 0 6px; }
.bar i, .fbar i { display: block; height: 100%; background: var(--accent); transition: width .15s linear; }
.bar i.error { background: var(--danger); }
.bar i.done { background: var(--ok); }
.meta { display: flex; gap: 10px; color: var(--muted); font-family: var(--font-mono); font-size: 11px; white-space: nowrap; overflow: hidden; }
.meta .err { overflow: hidden; text-overflow: ellipsis; }
.files { margin-top: 8px; max-height: 150px; overflow: auto; display: flex; flex-direction: column; gap: 5px; }
.file { display: grid; grid-template-columns: 1fr auto; gap: 2px 8px; font-family: var(--font-mono); font-size: 11px; color: var(--muted); }
.file.cur { color: var(--fg); }
.fname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; }
.fbar { grid-column: 1 / -1; height: 2px; }
.slide-enter-active, .slide-leave-active { transition: opacity .15s, transform .15s; }
.slide-enter-from, .slide-leave-to { opacity: 0; transform: translateY(6px); }
</style>
