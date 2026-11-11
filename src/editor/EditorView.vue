<script setup lang="ts">
// Built-in editor. Same font, theme tokens and transparent background as the terminal.
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { EditorState, Compartment } from "@codemirror/state";
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  drawSelection, highlightSpecialChars, rectangularSelection,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import {
  HighlightStyle, syntaxHighlighting, bracketMatching, indentOnInput, foldGutter, foldKeymap,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { editor, saveEditor, closeEditor } from "../stores/editor";
import { confirm } from "../stores/ui";
import { settings } from "../stores/settings";
import { languageFor } from "./languages";
import Icon from "../components/Icon.vue";

const host = ref<HTMLDivElement>();
const pos = ref({ line: 1, col: 1 });
const doc = computed(() => editor.doc);
const dirty = computed(() => !!doc.value && doc.value.content !== doc.value.saved);
let view: EditorView | null = null;
const lang = new Compartment();
const fontComp = new Compartment();

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.modifier, t.operatorKeyword, t.controlKeyword], color: "var(--ansi-5)" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--ansi-2)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--ansi-3)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--muted)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--ansi-4)" },
  { tag: [t.typeName, t.className, t.namespace], color: "var(--ansi-6)" },
  { tag: [t.propertyName, t.attributeName], color: "var(--ansi-12)" },
  { tag: [t.tagName, t.heading], color: "var(--ansi-1)", fontWeight: "600" },
  { tag: [t.definition(t.variableName), t.variableName], color: "var(--fg)" },
  { tag: [t.meta, t.processingInstruction], color: "var(--ansi-11)" },
  { tag: t.link, color: "var(--accent)", textDecoration: "underline" },
  { tag: t.invalid, color: "var(--danger)" },
]);

const theme = EditorView.theme({
  "&": { height: "100%", background: "transparent", color: "var(--fg)" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.45" },
  ".cm-content": { caretColor: "var(--fg)", padding: "8px 0" },
  ".cm-cursor": { borderLeftColor: "var(--fg)", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { background: "var(--selection) !important" },
  ".cm-gutters": { background: "transparent", color: "var(--muted)", border: "none", opacity: ".7" },
  ".cm-activeLine": { background: "rgb(var(--fg-rgb) / 0.04)" },
  ".cm-activeLineGutter": { background: "transparent", color: "var(--fg)" },
  ".cm-matchingBracket": { background: "rgb(var(--fg-rgb) / 0.12)", outline: "none" },
  ".cm-selectionMatch": { background: "rgb(var(--fg-rgb) / 0.08)" },
  ".cm-panels": { background: "rgb(var(--bg-rgb) / 0.97)", color: "var(--fg)" },
  ".cm-panels-bottom": { borderTop: "1px solid var(--line)" },
  ".cm-textfield": { background: "rgb(var(--fg-rgb) / 0.05)", border: "1px solid var(--line)", borderRadius: "4px", color: "var(--fg)" },
  ".cm-button": { background: "none", border: "1px solid var(--line)", borderRadius: "4px", color: "var(--fg)", backgroundImage: "none" },
  ".cm-foldPlaceholder": { background: "transparent", border: "1px solid var(--line)", color: "var(--muted)" },
}, { dark: true });

const fontTheme = () => EditorView.theme({ "&": { fontSize: settings.fontSize + "px" } });

async function requestClose() {
  if (dirty.value && !(await confirm({ title: "Kaydedilmemiş değişiklikler", message: "Kaydetmeden kapatılsın mı?", danger: true, okText: "Kapat" })))
    return view?.focus();
  closeEditor();
}

function build() {
  const d = doc.value;
  if (!host.value || !d || d.loading || d.error) return;
  view?.destroy();
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: d.content,
      extensions: [
        lineNumbers(), highlightActiveLineGutter(), foldGutter(), highlightSpecialChars(), history(),
        drawSelection(), indentOnInput(), bracketMatching(), rectangularSelection(),
        highlightActiveLine(), highlightSelectionMatches(), syntaxHighlighting(highlight),
        EditorState.tabSize.of(4), theme, fontComp.of(fontTheme()), lang.of([]),
        keymap.of([
          { key: "Mod-s", preventDefault: true, run: () => { saveEditor(); return true; } },
          { key: "Escape", run: () => { requestClose(); return true; } },
          ...defaultKeymap, ...historyKeymap, ...searchKeymap, ...foldKeymap, indentWithTab,
        ]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged && editor.doc) editor.doc.content = u.state.doc.toString();
          if (u.selectionSet || u.docChanged) {
            const head = u.state.selection.main.head;
            const line = u.state.doc.lineAt(head);
            pos.value = { line: line.number, col: head - line.from + 1 };
          }
        }),
      ],
    }),
  });
  view.focus();
  languageFor(d.path).then((ext) => { if (ext && view) view.dispatch({ effects: lang.reconfigure(ext) }); });
}

onMounted(build);
watch(() => [doc.value?.loading, doc.value?.path], build);
watch(() => settings.fontSize, () => view?.dispatch({ effects: fontComp.reconfigure(fontTheme()) }));
onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div v-if="doc" class="ed" @keydown.esc.stop>
    <div class="bar">
      <Icon name="edit" :size="14" class="muted" />
      <span class="path" :title="doc.path">{{ doc.path }}</span>
      <span v-if="doc.sudo" class="badge" data-tip="sudo ile kaydedilecek"><Icon name="shield" :size="12" /> sudo</span>
      <span v-if="!doc.exists && !doc.loading" class="badge">yeni</span>
      <span class="state" :class="{ dirty, saving: doc.saving }">
        <template v-if="doc.saving">kaydediliyor…</template>
        <template v-else-if="dirty">● değişti</template>
        <template v-else-if="!doc.loading && !doc.error"><Icon name="check" :size="12" /> kaydedildi</template>
      </span>
      <button class="ib" data-tip="Kaydet (⌘S)" data-tip-pos="bottom" :disabled="!dirty" @click="saveEditor()"><Icon name="check" /></button>
      <button class="ib" data-tip="Kapat (Esc)" data-tip-pos="bottom" @click="requestClose"><Icon name="x" /></button>
    </div>
    <div v-if="doc.loading" class="center"><div class="spinner" /></div>
    <div v-else-if="doc.error" class="center err"><Icon name="alert" :size="20" /> {{ doc.error }}</div>
    <div v-show="!doc.loading && !doc.error" ref="host" class="cm-host" />
    <div class="foot">
      <span>{{ pos.line }}:{{ pos.col }}</span>
      <span>{{ doc.tab.kind === 'local' ? 'local' : doc.tab.server.host }}</span>
    </div>
  </div>
</template>

<style scoped>
.ed { position: absolute; inset: 0; display: flex; flex-direction: column; font-family: var(--font-mono); }
.bar, .foot { display: flex; align-items: center; gap: 10px; padding: 0 10px; flex: none; font-size: 12px; color: var(--muted); }
.bar { height: 34px; border-bottom: 1px solid var(--line); }
.foot { height: 22px; border-top: 1px solid var(--line); justify-content: space-between; font-size: 11px; }
.path { flex: 1; min-width: 0; color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; }
.badge { display: inline-flex; align-items: center; gap: 3px; padding: 1px 6px; border: 1px solid var(--line); border-radius: 8px; font-size: 10.5px; }
.state { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; }
.state.dirty { color: var(--warn); }
.cm-host { flex: 1; min-height: 0; }
.center { flex: 1; display: flex; align-items: center; justify-content: center; gap: 8px; }
.err { color: var(--danger); }
.spinner { width: 16px; height: 16px; border-radius: 50%; border: 2px solid var(--line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
