# website

Forma's website: a static [Astro](https://astro.build) site with Tailwind CSS,
with the documentation (`docs`, an mdBook) published under `docs/`.
Released with everything else: a `v*` tag publishes it to GitHub Pages
(`.github/workflows/pages.yml`), at <https://germolinal.github.io/forma/>.

```sh
cd website
npm install
npm run dev        # the landing page alone, at http://localhost:4321/forma/
npm run format     # prettier; the build fails on unformatted files
./build.sh         # the whole site, docs included, into dist/
npm run preview    # serves dist/
```

The site is served under a path (`/forma/`), so internal links go through
`href()` in `src/config/site.ts`, never a bare `/...`. The design tokens in
`src/styles/globals.css` use shadcn's names; the button is shadcn's recipe
(`src/components/ui/`). No shadcn library is installed: nothing on the page
needs one yet.

The logo is in `public/` (`mark.svg`, also `favicon.svg`; `logo.svg`, mark and
wordmark) and `src/components/brand/`. The docs and the README use copies.
