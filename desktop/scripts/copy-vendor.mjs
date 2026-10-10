// node_modules の Leaflet を ui/vendor にコピーする（バンドラを使わないため）。
import { cpSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "node_modules/leaflet");
const dest = join(root, "ui/vendor/leaflet");
mkdirSync(dest, { recursive: true });
for (const f of ["dist/leaflet.js", "dist/leaflet.css", "LICENSE"]) {
  cpSync(join(src, f), join(dest, f.replace("dist/", "")));
}
cpSync(join(src, "dist/images"), join(dest, "images"), { recursive: true });
