// Lazy language support by file name. Only the needed grammar chunk is loaded.
import type { Extension } from "@codemirror/state";
import { StreamLanguage } from "@codemirror/language";

export async function languageFor(path: string): Promise<Extension | null> {
  const name = path.split("/").pop()!.toLowerCase();
  const ext = name.includes(".") ? name.split(".").pop()! : "";
  if (["js", "mjs", "cjs", "jsx"].includes(ext)) return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
  if (["ts", "tsx", "mts"].includes(ext)) return (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: ext === "tsx" });
  if (["json", "jsonc"].includes(ext)) return (await import("@codemirror/lang-json")).json();
  if (["py"].includes(ext)) return (await import("@codemirror/lang-python")).python();
  if (["yml", "yaml"].includes(ext)) return (await import("@codemirror/lang-yaml")).yaml();
  if (["md", "markdown"].includes(ext)) return (await import("@codemirror/lang-markdown")).markdown();
  if (["html", "htm", "vue", "xml", "svg"].includes(ext)) return (await import("@codemirror/lang-html")).html();
  if (["css", "scss", "less"].includes(ext)) return (await import("@codemirror/lang-css")).css();
  if (["sh", "bash", "zsh", "env"].includes(ext) || /^\.(bash|zsh)rc$|^\.profile$|^\.env/.test(name))
    return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/shell")).shell);
  if (name === "dockerfile" || ext === "dockerfile")
    return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile);
  if (ext === "toml") return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/toml")).toml);
  if (["ini", "conf", "cfg", "properties", "service"].includes(ext) || name.endsWith(".conf"))
    return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/properties")).properties);
  if (path.includes("/nginx/") || name === "nginx.conf")
    return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/nginx")).nginx);
  return null;
}
