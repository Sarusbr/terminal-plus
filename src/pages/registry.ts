// Page registry. Every right-bar page is a module described here; adding a page
// (e.g. Nginx) means writing a component and adding one entry.
import { defineAsyncComponent, type Component } from "vue";

export interface PageDef {
  id: string;
  icon: string;
  title: string;
  /** Lazy component; null for the terminal itself. */
  component: Component | null;
  /** Needs a connected, idle shell (runs commands in the PTY). */
  needsShell?: boolean;
  /** Pinned to the bottom of the bar. */
  bottom?: boolean;
}

export const pages: PageDef[] = [
  { id: "terminal", icon: "terminal", title: "Terminal", component: null },
  { id: "files", icon: "folder", title: "Dosyalar", component: defineAsyncComponent(() => import("./files/FilesPage.vue")), needsShell: true },
  { id: "docker", icon: "box", title: "Docker", component: defineAsyncComponent(() => import("./docker/DockerPage.vue")), needsShell: true },
  { id: "system", icon: "gauge", title: "Sistem", component: defineAsyncComponent(() => import("./system/SystemPage.vue")), needsShell: true },
  { id: "processes", icon: "cpu", title: "Süreçler", component: defineAsyncComponent(() => import("./processes/ProcessesPage.vue")), needsShell: true },
  { id: "ports", icon: "ports", title: "Portlar", component: defineAsyncComponent(() => import("./ports/PortsPage.vue")), needsShell: true },
  { id: "logs", icon: "scroll", title: "Loglar", component: defineAsyncComponent(() => import("./logs/LogsPage.vue")), needsShell: true },
  { id: "services", icon: "services", title: "Servisler", component: defineAsyncComponent(() => import("./services/ServicesPage.vue")), needsShell: true },
  { id: "servers", icon: "server", title: "Sunucular", component: defineAsyncComponent(() => import("./ServersPage.vue")) },
  { id: "settings", icon: "cog", title: "Ayarlar", component: defineAsyncComponent(() => import("./SettingsPage.vue")), bottom: true },
];
