import { defineConfig } from "vite";
import react from "@vitejs/plugin-react-swc";
import tailwindcss from "@tailwindcss/vite";
import wasm from "vite-plugin-wasm";

export default defineConfig({
  plugins: [react(), tailwindcss(), wasm()],
  optimizeDeps: {
    exclude: ["tryke_wasm"],
  },
  worker: {
    plugins: () => [wasm()],
  },
});
