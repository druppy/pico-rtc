# Vendored: PicoCSS 2.1.1 SCSS sources

**Do not hand-edit these files.** This is upstream code, kept byte-for-byte as
published so it can be replaced wholesale on upgrade.

| Field              | Value                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------- |
| Package            | `@picocss/pico`                                                                                         |
| Version            | **2.1.1** (npm `dist-tags.latest` at vendor time)                                                       |
| License            | MIT — see [`LICENSE.md`](./LICENSE.md)                                                                   |
| Upstream           | https://github.com/picocss/pico                                                                          |
| Tarball            | https://registry.npmjs.org/@picocss/pico/-/pico-2.1.1.tgz                                                |
| Vendored here      | `package/scss/**/*.scss` only — 54 files, 180,853 bytes                                                  |
| Deliberately out   | `css/` (239 prebuilt CSS files, ~19 MB), `scss/postcss.config.js`, docs, `.github/`, `scripts/`          |

The consumer is `../main.scss`, which does `@use "pico/pico" with (...)`. Pico's
own entry graph (`pico.scss` → `_index.scss` → the partials) is untouched, so the
`@use`/`@forward` relative paths keep resolving inside this directory.

## Upgrading

`css/` and `postcss.config.js` are the only things stripped; everything else
under `scss/` is vendored as-is. Re-vendor with:

```sh
rm -rf style/pico && mkdir -p style/pico
curl -sL https://registry.npmjs.org/@picocss/pico/-/pico-2.1.1.tgz -o /tmp/pico.tgz
tar xzf /tmp/pico.tgz -C style/pico --wildcards --strip-components=2 'package/scss/*.scss'
tar xzf /tmp/pico.tgz -C style/pico --wildcards --strip-components=1 'package/LICENSE.md'
```

Then bump the version above, and re-check `../main.scss` still compiles
(`sass style/main.scss /tmp/out.css` from the repo root).

## Compiler floor

This graph needs a **modern Dart Sass**. It is not portable to older Sass, nor to
alternative implementations.

| Feature                    | Requires              | Used in                                                  |
| -------------------------- | --------------------- | -------------------------------------------------------- |
| `color.channel()`          | Dart Sass ≥ 1.79.0    | `helpers/_functions.scss`, `colors/utilities/_utils.scss`  |
| `map.deep-merge()`         | Dart Sass ≥ 1.27.0    | `_settings.scss`                                           |
| `@use`/`@forward` + `with` | Dart Sass only        | the whole graph; `as *` in 36 files                        |

Verified by compiling: Dart Sass 1.86.0 (cargo-leptos' default
`LEPTOS_SASS_VERSION`) is clean; Dart Sass 1.78.0 dies on `Undefined function` at
`color.channel`. Do not lower `LEPTOS_SASS_VERSION` below 1.79.

## Known upstream deprecation

On a newer compiler Pico 2.1.1 emits **one** deprecation warning per build:
`[if-function]`, from `components/_modal.scss:36`. It is upstream's, not ours.
At Dart Sass 1.86.0 the graph is silent, so the pinned version costs nothing.
