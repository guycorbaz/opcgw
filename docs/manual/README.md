# opcgw User Manual

The comprehensive user manual for opcgw, authored in **LaTeX** and built to a
polished PDF with LuaLaTeX.

> **Format note:** The manual was migrated from DocBook 4.5 XML to LaTeX on
> 2026-06-27 (GH #145). LaTeX is now the canonical source format; the DocBook
> XML and its xsltproc/dblatex toolchain have been retired.

## Files (`latex/`)

- **`main.tex`** — Document shell: class options, title page, table of contents.
- **`preamble.tex`** — The modern style: fonts (Lato / Noto Serif / DejaVu Sans
  Mono), brand palette, KOMA-Script heading layout, `tcolorbox` admonitions,
  `booktabs` tables, framed `listings` code blocks, `hyperref`, Unicode mappings.
- **`body.tex`** — The manual content (chapters + appendices). **This is the
  file you edit** to change the manual text.
- **`build.sh`** — Build driver: converts figure sources to vector PDF, stamps
  the version from `Cargo.toml`, runs `latexmk -lualatex`.

Figures are generated at build time from the shared sources in `../images/`
(architecture diagram via Graphviz `dot`; screenshots via `mutool`) and
`../logo/` — they are not committed (`latex/.gitignore`).

## Building

From this directory:

```bash
make pdf          # → latex/opcgw-user-manual.pdf
make clean        # remove generated artifacts
make print-deps   # show build-tool versions
```

### Prerequisites (Debian/Ubuntu)

```bash
sudo apt-get install -y texlive-luatex texlive-latex-extra texlive-fonts-extra \
                        fonts-lato fonts-noto fonts-dejavu graphviz mupdf-tools
```

## Content

- **Overview** — what opcgw is, key features, system architecture, data flow.
- **System Requirements** — hardware, software, network.
- **Installation** — Docker and from source; first-run prerequisites.
- **Configuration** — first-run setup wizard, web-UI inventory pickers, drift
  reconciliation, settings reference (`[global]`, `[logging]`, `[chirpstack]`,
  `[opcua]`, command delivery, the `[[application]]` hierarchy), config storage
  and migration, environment-variable overrides.
- **Daily Operation and Maintenance** — status dashboard, the OPC UA Gateway
  folder, stale-data status codes, KPIs, command-queue monitoring, logs and
  performance budgets, database maintenance, pruning and retention.
- **Troubleshooting** — symptom cookbook.
- **Upgrade and Migration** — version-to-version upgrade procedures.
- **Appendices** — configuration reference, environment-variable reference,
  OPC UA address space, operation names, audit/diagnostic events, glossary.

## Editing

Edit `latex/body.tex` for content and `latex/preamble.tex` for styling, then run
`make pdf`. Cross-references use `\hyperref[label]{...}` against the `\label{…}`
anchors carried over from the original section IDs (`ch-…`, `app-…`).

## License

This documentation is licensed under MIT OR Apache-2.0, same as the opcgw project.

## Contact

- GitHub issues: https://github.com/guycorbaz/opcgw/issues
- Guy Corbaz (gcorbaz@gmail.com)
