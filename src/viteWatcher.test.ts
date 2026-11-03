import { createServer, resolveConfig } from "vite";
import { describe, expect, it } from "vitest";
import viteConfig from "../vite.config";
import tauriConfig from "../src-tauri/tauri.conf.json";

describe("frontend watcher boundary", () => {
  it("serves the fixed desktop development URL without silently moving ports", async () => {
    const config = await resolveConfig({}, "serve");
    const desktopUrl = new URL(tauriConfig.build.devUrl);
    expect(config.server.port).toBe(Number(desktopUrl.port));
    expect(config.server.strictPort).toBe(true);
  });

  it("watches frontend source while excluding the native tree", async () => {
    const configuredWatch = viteConfig.server?.watch;
    expect(configuredWatch).toEqual({ ignored: ["**/src-tauri/**"] });

    const server = await createServer({
      ...viteConfig,
      server: {
        ...viteConfig.server,
        middlewareMode: true,
        hmr: false,
      },
    });

    try {
      await new Promise((resolve) => setTimeout(resolve, 500));
      const watchedDirectories = Object.keys(server.watcher.getWatched()).map((path) =>
        path.replaceAll("\\", "/"),
      );

      expect(watchedDirectories.some((path) => path.endsWith("/src/app"))).toBe(true);
      expect(watchedDirectories.some((path) => path.includes("/src-tauri/"))).toBe(false);
    } finally {
      await server.close();
    }
  });
});
