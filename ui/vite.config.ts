import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 桌面壳的 Vite 配置：固定端口（devUrl 约定），构建目标对齐 macOS WebKit。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/crates/**", "**/target/**"],
    },
  },
  build: {
    target: "safari15",
    sourcemap: false,
  },
});
