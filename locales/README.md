# freewipe language packs

English is built into freewipe. Every other interface language is a **language
pack**: one JSON file per language in this folder.

```
locales/
  README.md    this file
  en.json      English — the reference (also compiled into the program)
  de.json      Deutsch
  es.json      Español
  fr.json      Français
  pl.json      Polski
  ru.json      Русский
  uk.json      Українська
```

Keep the `locales` folder next to `freewipe.exe` / `freewipe-gui.exe`. freewipe also
reads packs from `%APPDATA%\freewipe\locales` and from the folder named by the
`FREEWIPE_LOCALES_DIR` environment variable; later folders override earlier ones
key by key. Remove a file and that language disappears from the list; add one
and it appears (Settings → General → Interface language, or `--lang <code>` on
the command line). `freewipe languages` lists the installed packs, where they
were found and how complete they are.

## Format

* **File name**: the language code — ISO 639-1, lower case: `de.json`,
  `pt.json`. Region suffixes are accepted and ignored (`pt-BR.json` → `pt`).
* **Encoding**: UTF-8, no byte-order mark.
* **Content**: one flat JSON object, `"key": "text"`, the same keys as
  `en.json`. Order does not matter.
* **Metadata keys** (start with `_`, not shown in the program as text):

  | Key            | Meaning                                         |
  |----------------|-------------------------------------------------|
  | `_name`        | the language's own name, shown in the language list (`Deutsch`) |
  | `_name_en`     | its English name (`German`)                     |
  | `_translators` | who translated it                               |
  | `_freewipe`       | the freewipe version the pack was written for      |

* **Placeholders** in braces — `{n}`, `{path}`, `{method}` — are filled in by
  freewipe. Keep every placeholder of the English text, unchanged; you may move
  them within the sentence. `help.template` uses the command-line help
  placeholders (`{usage}`, `{all-args}`, …) and must keep them all.
* `\n` is a line break. Keep leading/trailing spaces where the English text
  has them (e.g. `cli.with_verify`, `cli.ok`).
* Missing keys fall back to English, so a partial pack works — it is shown
  with its completeness, e.g. `Italiano (it, 85%)`.

## Adding or updating a language

1. Copy `en.json` to `<code>.json` and translate the values (not the keys).
2. Set `_name`, `_name_en`, `_translators` and `_freewipe`.
3. Put the file into `locales` and choose the language in Settings → General.
   Context-menu labels change after *Apply* in Settings → Integration.
4. When contributing it to the project, `cargo test` checks that every pack
   has exactly the English keys and the same placeholders.
