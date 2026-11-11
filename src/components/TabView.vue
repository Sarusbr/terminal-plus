<script setup lang="ts">
import { connect, closeTab, type Tab } from "../stores/tabs";
import PaneTree from "./PaneTree.vue";
import Icon from "./Icon.vue";

defineProps<{ tab: Tab }>();
</script>

<template>
  <div class="tabview">
    <PaneTree v-if="tab.gen > 0" :tab="tab" />
    <div v-if="tab.status !== 'connected'" class="overlay" :class="{ dim: tab.gen > 0 }">
      <div class="card">
        <template v-if="tab.status === 'connecting'">
          <div class="spinner" />
          <div class="muted">{{ tab.server.user }}@{{ tab.server.host }}:{{ tab.server.port }}</div>
        </template>
        <template v-else>
          <Icon :name="tab.status === 'ended' ? 'terminal' : 'alert'" :size="22" />
          <div>
            {{ tab.status === 'ended' ? 'Oturum sonlandı' : tab.status === 'disconnected' ? 'Bağlantı koptu' : 'Bağlanılamadı' }}
          </div>
          <div v-if="tab.error" class="err">{{ tab.error }}</div>
          <div class="row">
            <button class="btn primary" @click="connect(tab)"><Icon name="refresh" :size="14" /> Yeniden bağlan</button>
            <button class="btn" @click="closeTab(tab.id)"><Icon name="x" :size="14" /> Kapat</button>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tabview { position: absolute; inset: 0; }
.overlay { position: absolute; inset: 0; display: grid; place-items: center; z-index: 10; }
.overlay.dim { background: rgb(var(--bg-rgb) / 0.55); }
.card { display: flex; flex-direction: column; align-items: center; gap: 10px; max-width: 460px; text-align: center; color: var(--fg); }
.err { color: var(--danger); font-size: 12px; word-break: break-word; }
.row { display: flex; gap: 8px; margin-top: 4px; }
.spinner {
  width: 18px; height: 18px; border-radius: 50%;
  border: 2px solid var(--line); border-top-color: var(--accent);
  animation: spin .8s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }
</style>
