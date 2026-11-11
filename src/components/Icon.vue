<script setup lang="ts">
// Hand-drawn 24×24 stroke icons. No icon library.
defineProps<{ name: string; size?: number }>();

const paths: Record<string, string> = {
  terminal: "M4 6l5 5-5 5M11 17h8",
  server: "M4 5h16v6H4zM4 13h16v6H4zM7.5 8h.01M7.5 16h.01",
  folder: "M3 7a1 1 0 011-1h5l2 2h9a1 1 0 011 1v9a1 1 0 01-1 1H4a1 1 0 01-1-1z",
  box: "M12 3l8 4.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9",
  activity: "M3 12h4l3-8 4 16 3-8h4",
  list: "M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01",
  plug: "M9 3v5M15 3v5M6 8h12v3a6 6 0 01-12 0zM12 17v4",
  scroll: "M6 4h11a2 2 0 012 2v12a2 2 0 01-2 2H7M6 4a2 2 0 00-2 2v1h4M6 4a2 2 0 012 2v12a2 2 0 002 2M11 9h5M11 13h5",
  cog: "M12 15a3 3 0 100-6 3 3 0 000 6zM12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1",
  plus: "M12 5v14M5 12h14",
  x: "M6 6l12 12M18 6L6 18",
  edit: "M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4",
  trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
  link: "M10 14a4 4 0 006 0l3-3a4 4 0 00-6-6l-1 1M14 10a4 4 0 00-6 0l-3 3a4 4 0 006 6l1-1",
  import: "M12 3v12M7 10l5 5 5-5M4 17v3h16v-3",
  search: "M11 18a7 7 0 100-14 7 7 0 000 14zM20 20l-4-4",
  key: "M15 9a3 3 0 11-6 0 3 3 0 016 0zM13.5 11.5L20 18M17 15l2-2M18.5 16.5l1.5-1.5",
  lock: "M6 11h12v9H6zM8 11V8a4 4 0 018 0v3",
  refresh: "M20 11a8 8 0 10-2.3 5.7M20 4v7h-7",
  splitRow: "M4 5h16v14H4zM12 5v14",
  splitCol: "M4 5h16v14H4zM4 12h16",
  up: "M6 15l6-6 6 6",
  down: "M6 9l6 6 6-6",
  chevron: "M9 6l6 6-6 6",
  shield: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z",
  alert: "M12 4l9 16H3zM12 10v4M12 17h.01",
  check: "M5 12l5 5L20 7",
  tag: "M3 12V4h8l10 10-8 8zM7.5 8h.01",
  file: "M6 3h8l4 4v14H6zM14 3v4h4",
  filePlus: "M6 3h8l4 4v14H6zM14 3v4h4M12 11v6M9 14h6",
  folderPlus: "M3 7a1 1 0 011-1h5l2 2h9a1 1 0 011 1v9a1 1 0 01-1 1H4a1 1 0 01-1-1zM12 11v5M9.5 13.5h5",
  fileLink: "M6 3h8l4 4v14H6zM14 3v4h4M10 15l4-4M11 11h3v3",
  copy: "M8 8h12v12H8zM4 16V4h12",
  cut: "M6 7a2 2 0 100-4 2 2 0 000 4zM6 21a2 2 0 100-4 2 2 0 000 4zM7.5 6.5L20 18M7.5 17.5L20 6",
  clipboard: "M8 4h8v3H8zM8 5.5H5V21h14V5.5h-3",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12zM12 15a3 3 0 100-6 3 3 0 000 6z",
  eyeOff: "M3 3l18 18M10.6 5.1A10 10 0 0112 5c6 0 10 7 10 7a17 17 0 01-3.2 3.9M6.3 6.3C3.7 8 2 12 2 12s4 7 10 7a9.6 9.6 0 004.2-1M9.9 9.9a3 3 0 004.2 4.2",
  arrowUp: "M12 19V5M6 11l6-6 6 6",
  home: "M4 11l8-7 8 7v9h-5v-6H9v6H4z",
  play: "M7 4l13 8-13 8z",
  stop: "M6 6h12v12H6z",
  pause: "M8 5v14M16 5v14",
  zap: "M13 2L4 14h7l-1 8 9-12h-7z",
  info: "M12 22a10 10 0 100-20 10 10 0 000 20zM12 11v6M12 7.5h.01",
  layers: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5M3 17.5l9 5 9-5",
  database: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  network: "M12 3v6M5 21v-4h14v4M12 9a3 3 0 100 6 3 3 0 000-6zM12 15v2",
  broom: "M15 3l-5 9M6 12h8l2 9H4zM9 16v5M12 16v5",
  chmod: "M6 11h12v9H6zM8 11V8a4 4 0 017.5-2M12 14v3",
  pull: "M12 3v12M7 10l5 5 5-5M4 17v3h16v-3",
  gauge: "M4 18a8 8 0 1116 0M12 14l4-5M12 14a1.5 1.5 0 100 3 1.5 1.5 0 000-3z",
  cpu: "M7 7h10v10H7zM10 10h4v4h-4zM9 3v4M15 3v4M9 17v4M15 17v4M3 9h4M3 15h4M17 9h4M17 15h4",
  ports: "M5 12h3M16 12h3M8 8h8v8H8zM12 4v4M12 16v4",
  services: "M12 3l2 3h3l-1 3 2 3-3 1-1 3-3-1-3 1-1-3-3-1 2-3-1-3h3zM12 10a2 2 0 100 4 2 2 0 000-4z",
  skull: "M12 3a7 7 0 00-7 7c0 2.5 1.3 4.2 3 5.2V19h8v-3.8c1.7-1 3-2.7 3-5.2a7 7 0 00-7-7zM9.5 11h.01M14.5 11h.01M10 19v2M14 19v2",
};
</script>

<template>
  <svg
    :width="size ?? 16" :height="size ?? 16" viewBox="0 0 24 24" fill="none"
    stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"
    aria-hidden="true" class="icon"
  >
    <path :d="paths[name] ?? ''" />
  </svg>
</template>

<style scoped>
.icon { flex: none; display: block; }
</style>
