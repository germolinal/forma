// @ts-check
/// <reference types="node" />
import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

// Published on GitHub Pages as a project site: https://germolinal.github.io/forma/.
// The Pages workflow passes the real origin and path (actions/configure-pages),
// so a custom domain needs no change here.
const site = process.env.SITE_URL ?? "https://germolinal.github.io";
const base = process.env.BASE_PATH ?? "/forma";

// https://astro.build/config
export default defineConfig({
  site,
  base,
  output: "static",
  compressHTML: true,
  vite: {
    plugins: [tailwindcss()],
  },
});
