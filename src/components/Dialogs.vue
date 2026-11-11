<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { ui, closeDialog } from "../stores/ui";
import Icon from "./Icon.vue";

// Only the top-most dialog is shown; the rest wait in the queue.
const d = computed(() => ui.dialogs[0]);
const value = ref("");
const remember = ref(true);
const input = ref<HTMLInputElement>();
const okBtn = ref<HTMLButtonElement>();

watch(d, (cur) => {
  if (!cur) return;
  value.value = cur.kind === "prompt" ? cur.opts.value ?? "" : "";
  remember.value = true;
  nextTick(() => (input.value ?? okBtn.value)?.focus());
}, { immediate: true });

function ok() {
  const cur = d.value;
  if (!cur) return;
  if (cur.kind === "choose") return;
  if (cur.kind === "prompt") closeDialog(cur, { value: value.value, remember: remember.value });
  else closeDialog(cur, true);
  value.value = "";
}
function pick(v: string) {
  const cur = d.value;
  if (cur?.kind === "choose") closeDialog(cur, v);
}
function cancel() {
  const cur = d.value;
  if (!cur) return;
  closeDialog(cur, cur.kind === "prompt" || cur.kind === "choose" ? null : false);
  value.value = "";
}
</script>

<template>
  <div v-if="d" class="backdrop" @keydown.esc.stop="cancel" @mousedown.self="cancel">
    <form class="dialog" @submit.prevent="ok">
      <template v-if="d.kind === 'prompt'">
        <div class="head"><Icon :name="d.opts.secret ? 'lock' : 'edit'" /> {{ d.opts.title }}</div>
        <div v-if="d.opts.message" class="msg">{{ d.opts.message }}</div>
        <input
          ref="input" v-model="value" :type="d.opts.secret ? 'password' : 'text'"
          :placeholder="d.opts.placeholder" spellcheck="false" autocomplete="off"
        />
        <label v-if="d.opts.remember" class="check">
          <input v-model="remember" type="checkbox" /> Keychain'e kaydet
        </label>
      </template>

      <template v-else-if="d.kind === 'choose'">
        <div class="head"><Icon name="list" /> {{ d.opts.title }}</div>
        <div v-if="d.opts.message" class="msg">{{ d.opts.message }}</div>
        <div class="choices">
          <button v-for="(o, i) in d.opts.options" :key="o" type="button" class="choice" :ref="(el) => { if (i === 0) okBtn = el as HTMLButtonElement }" @click="pick(o)">{{ o }}</button>
        </div>
      </template>

      <template v-else-if="d.kind === 'confirm'">
        <div class="head"><Icon :name="d.opts.danger ? 'alert' : 'check'" /> {{ d.opts.title }}</div>
        <div v-if="d.opts.message" class="msg">{{ d.opts.message }}</div>
      </template>

      <template v-else>
        <div class="head"><Icon name="shield" /> Bilinmeyen sunucu</div>
        <div class="msg">
          <b>{{ d.req.host }}:{{ d.req.port }}</b> ilk kez bağlanılıyor. Parmak izini doğrula:
        </div>
        <div class="fp">
          <span class="muted">{{ d.req.algorithm }}</span>
          <code>{{ d.req.fingerprint }}</code>
        </div>
        <div class="msg muted">Onaylarsan ~/.ssh/known_hosts dosyasına eklenir.</div>
      </template>

      <div class="actions">
        <button type="button" class="btn" @click="cancel">Vazgeç</button>
        <button
          v-if="d.kind !== 'choose'"
          ref="okBtn" type="submit" class="btn primary"
          :class="{ danger: d.kind === 'confirm' && d.opts.danger }"
        >
          {{ d.kind === 'hostkey' ? 'Güven ve bağlan' : d.kind === 'confirm' ? (d.opts.okText ?? 'Tamam') : 'Tamam' }}
        </button>
      </div>
    </form>
  </div>
</template>

<style scoped>
.backdrop {
  position: fixed; inset: 0; z-index: 100; display: grid; place-items: center;
  background: rgb(0 0 0 / 0.35); animation: fade .1s ease-out;
}
.dialog {
  width: min(400px, calc(100vw - 32px)); display: flex; flex-direction: column; gap: 12px;
  padding: 16px; border-radius: 10px; border: 1px solid var(--line);
  background: rgb(var(--bg-rgb) / 0.98); box-shadow: 0 12px 40px rgb(0 0 0 / 0.4);
  animation: pop .12s ease-out;
}
.head { display: flex; align-items: center; gap: 8px; font-weight: 600; }
.msg { color: var(--fg); font-size: 12.5px; line-height: 1.5; white-space: pre-line; word-break: break-word; }
.choices { display: flex; flex-direction: column; gap: 4px; max-height: 260px; overflow: auto; }
.choice { text-align: left; padding: 7px 10px; border-radius: 6px; border: 1px solid var(--line); font-family: var(--font-mono); font-size: 12.5px; }
.choice:hover, .choice:focus { background: rgb(var(--fg-rgb) / 0.07); outline: none; border-color: var(--accent); }
.msg.muted { color: var(--muted); }
.fp { display: flex; flex-direction: column; gap: 4px; padding: 10px; border-radius: 6px; background: rgb(var(--fg-rgb) / 0.05); font-size: 12px; }
.fp code { font-family: var(--font-mono); word-break: break-all; color: var(--accent); }
.check { display: flex; gap: 8px; align-items: center; font-size: 12px; color: var(--muted); }
.actions { display: flex; justify-content: flex-end; gap: 8px; }
@keyframes fade { from { opacity: 0; } }
@keyframes pop { from { transform: scale(.97); opacity: 0; } }
</style>
