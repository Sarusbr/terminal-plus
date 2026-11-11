<script setup lang="ts">
import { settings, platform, resetSettings, currentTheme } from "../stores/settings";
import { localShells, loadLocalShells } from "../stores/tabs";
if (!localShells.list.length) loadLocalShells();
import { themes } from "../lib/themes";
import Icon from "../components/Icon.vue";

const colorKeys = [["bg", "Arka plan"], ["fg", "Yazı"], ["accent", "Vurgu"], ["cursor", "İmleç"], ["selection", "Seçim"]] as const;
type ColorKey = (typeof colorKeys)[number][0];
const colorOf = (k: ColorKey) => settings.custom[k] || currentTheme()[k];
function setColor(k: ColorKey, v: string) { settings.custom = { ...settings.custom, [k]: v }; }
function resetColor(k: ColorKey) { const c = { ...settings.custom }; delete c[k]; settings.custom = c; }
const hasCustom = () => Object.values(settings.custom).some(Boolean);

const mod = platform.os === "macos" ? "⌘" : "Ctrl+Shift+";
const shortcuts: [string, string][] = [
  [`${mod}T`, "Yeni yerel terminal"],
  [platform.os === "macos" ? "⌘O" : "—", "Sunucular"],
  [platform.os === "macos" ? "⌘," : "Ctrl+,", "Ayarlar"],
  [`${mod}W`, "Paneli / sekmeyi kapat"],
  [`${mod}D`, "Dikey böl"],
  [platform.os === "macos" ? "⌘⇧D" : "Ctrl+Shift+E", "Yatay böl"],
  [`${mod}C / ${mod}V`, "Kopyala / yapıştır"],
  [`${mod}F`, "Terminalde ara"],
  [`${mod}K`, "Ekranı temizle"],
  [`${platform.os === "macos" ? "⌘" : "Ctrl+"}1…9`, "Sekmeye git"],
  [`${platform.os === "macos" ? "⌘⇧[ ]" : "Ctrl+Tab"}`, "Önceki / sonraki sekme"],
  [`${platform.os === "macos" ? "⌘" : "Ctrl+"}+ / −`, "Yazı boyutu"],
];
</script>

<template>
  <div class="page">
    <section>
      <h4>Tema</h4>
      <div class="themes">
        <button
          v-for="(t, key) in themes" :key="key" class="theme" :class="{ on: settings.theme === key }"
          :style="{ background: t.bg, color: t.fg }" @click="settings.theme = key"
        >
          <span class="sw">
            <i v-for="c in t.ansi.slice(1, 7)" :key="c" :style="{ background: c }" />
          </span>
          {{ t.name }}
        </button>
      </div>
    </section>

    <section>
      <h4>Renkler <button v-if="hasCustom()" class="link" @click="settings.custom = {}">temaya dön</button></h4>
      <div class="colors">
        <label v-for="[k, l] in colorKeys" :key="k" class="color" :class="{ custom: !!settings.custom[k] }">
          <input type="color" :value="colorOf(k)" @input="setColor(k, ($event.target as HTMLInputElement).value)" />
          <span>{{ l }}</span>
          <button v-if="settings.custom[k]" class="ib" data-tip="Sıfırla" @click.prevent="resetColor(k)"><Icon name="x" :size="11" /></button>
        </label>
      </div>
    </section>

    <section>
      <h4>Yazı</h4>
      <label class="row"><span>Font</span><input v-model.lazy="settings.fontFamily" spellcheck="false" /></label>
      <label class="row"><span>Boyut</span><input v-model.number="settings.fontSize" type="range" min="9" max="24" /><b>{{ settings.fontSize }}</b></label>
      <label class="row"><span>Ligature <i class="muted">(deneysel)</i></span><input v-model="settings.ligatures" type="checkbox" /></label>
      <label class="row"><span>Satır yüksekliği</span><input v-model.number="settings.lineHeight" type="range" min="1" max="1.8" step="0.05" /><b>{{ settings.lineHeight.toFixed(2) }}</b></label>
    </section>

    <section>
      <h4>Pencere</h4>
      <label class="row"><span>Opaklık</span><input v-model.number="settings.opacity" type="range" min="0.3" max="1" step="0.01" /><b>{{ Math.round(settings.opacity * 100) }}%</b></label>
      <label class="row"><span>Blur</span><input v-model="settings.blur" type="checkbox" /></label>
      <label class="row"><span>Blur yoğunluğu</span>
        <select v-model="settings.blurLevel" :disabled="!settings.blur">
          <option value="light">hafif</option><option value="medium">orta</option><option value="strong">yoğun</option>
        </select>
        <i v-if="platform.os === 'linux'" class="muted note">Linux'ta pencere yöneticisine bağlı</i>
      </label>
    </section>

    <section>
      <h4>Terminal</h4>
      <label v-if="localShells.list.length > 1" class="row"><span>Varsayılan shell</span>
        <select v-model="settings.localShell">
          <option v-for="(s, i) in localShells.list" :key="s.id" :value="i === 0 ? '' : s.id">{{ s.name }}</option>
        </select>
      </label>
      <label class="row"><span>İmleç</span>
        <select v-model="settings.cursorStyle">
          <option value="block">blok</option><option value="bar">çizgi</option><option value="underline">alt çizgi</option>
        </select>
      </label>
      <label class="row"><span>İmleç yanıp sönsün</span><input v-model="settings.cursorBlink" type="checkbox" /></label>
      <label class="row"><span>Scrollback</span><input v-model.number="settings.scrollback" type="number" min="500" max="200000" step="500" /></label>
      <label class="row"><span>Zil</span>
        <select v-model="settings.bell"><option value="visual">görsel</option><option value="none">kapalı</option></select>
      </label>
    </section>

    <section>
      <h4>Kısayollar</h4>
      <div v-for="[k, d] in shortcuts" :key="k" class="row kb"><span>{{ d }}</span><kbd>{{ k }}</kbd></div>
    </section>

    <button class="btn reset" @click="resetSettings"><Icon name="refresh" :size="14" /> Varsayılanlar</button>
  </div>
</template>

<style scoped>
.page { height: 100%; overflow: auto; padding: 8px 22px 30px; max-width: 640px; }
h4 { margin: 18px 0 8px; font-size: 11px; font-weight: 500; color: var(--muted); text-transform: uppercase; letter-spacing: .06em; }
.themes { display: grid; grid-template-columns: repeat(auto-fill, minmax(130px, 1fr)); gap: 8px; }
.theme { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; padding: 10px; border-radius: 6px; border: 1px solid var(--line); font-size: 12px; }
.theme.on { outline: 2px solid var(--accent); outline-offset: 1px; }
.sw { display: flex; gap: 3px; }
.sw i { width: 10px; height: 10px; border-radius: 2px; }
.row { display: flex; align-items: center; gap: 12px; padding: 5px 0; font-size: 12.5px; }
.row > span { width: 150px; color: var(--fg); flex: none; }
.row input[type="range"] { flex: 1; max-width: 240px; }
.row input:not([type]), .row input[type="number"] { flex: 1; max-width: 320px; }
.row b { font-weight: 400; color: var(--muted); font-family: var(--font-mono); font-size: 12px; }
.kb { justify-content: space-between; max-width: 420px; }
kbd { font-family: var(--font-mono); font-size: 11.5px; color: var(--muted); }
.reset { margin-top: 24px; }
.link { margin-left: 8px; font-size: 10.5px; color: var(--accent); text-transform: none; letter-spacing: 0; }
.colors { display: flex; flex-wrap: wrap; gap: 8px; }
.color { display: flex; align-items: center; gap: 6px; padding: 4px 6px 4px 4px; border: 1px solid var(--line); border-radius: 6px; font-size: 12px; }
.color.custom { border-color: var(--accent); }
.color input { width: 22px; height: 22px; padding: 0; border: 0; background: none; border-radius: 4px; cursor: pointer; }
.color .ib { width: 18px; height: 18px; }
.note { font-size: 11px; }
i { font-style: normal; }
</style>
