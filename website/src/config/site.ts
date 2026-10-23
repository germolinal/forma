/**
 * Site-wide configuration. Everything here is meant to be edited without
 * touching a component.
 */

/** Where the site is served from: "/forma/" on GitHub Pages, "/" on a domain. */
const BASE = import.meta.env.BASE_URL.replace(/\/?$/, "/");

/**
 * A link inside this site, with the base path in front. Every internal link
 * goes through here, so the site works under any path.
 */
export const href = (path = "") => BASE + path.replace(/^\//, "");

const REPO_URL = "https://github.com/germolinal/forma";

export const site = {
  name: "Forma",
  tagline: "An AI agent runtime",
  description:
    "Forma is a runtime for production AI agents. Define the agent in YAML, evaluate it, and serve it over HTTP, the web, WhatsApp or Telegram.",
  repo: REPO_URL,
} as const;

/** Links into the documentation (an mdBook, published under docs/). */
export const docs = {
  home: href("docs/"),
  gettingStarted: href("docs/how-to/getting-started.html"),
  deployment: href("docs/how-to/deployment.html"),
  evals: href("docs/documentation/evals.html"),
} as const;

/** Links into the repository. */
export const links = {
  bench: REPO_URL + "/tree/main/forma/bench",
} as const;

/** Primary navigation, rendered in the header. */
export const nav: { label: string; href: string }[] = [
  { label: "Docs", href: docs.home },
  { label: "Get started", href: docs.gettingStarted },
  { label: "GitHub", href: REPO_URL },
];
