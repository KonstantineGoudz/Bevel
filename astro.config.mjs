import { defineConfig } from "astro/config";

export default defineConfig({
  server: { port: 4099, host: true },
  redirects: {
    "/decisions": "/mechanics/",
    "/crafting": "/mechanics/crafting/",
  },
});
