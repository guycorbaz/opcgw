# opcgw User Manual

This directory contains the comprehensive user manual for opcgw in DocBook 5.2 XML format.

## Files

- **opcgw-user-manual.xml** — Complete user manual covering:
  - Installation (Docker and source)
  - Configuration guide
  - Operation and monitoring
  - Troubleshooting
  - Maintenance procedures
  - Appendices with references

- **index.xml** — Documentation index and build instructions

- **Sans_titre1.xml** — Template file (placeholder, can be removed)

- **opcgw.xpr** — Oxygen XML Editor project file

## Reading the Manual

### Online
The XML files can be viewed in any text editor or specialized XML editors like:
- Oxygen XML Editor
- VS Code with XML support
- Any web browser with DocBook stylesheets

### Building Output Formats

#### HTML (Single Page)
```bash
xsltproc --output opcgw-user-manual.html \
  /usr/share/xml/docbook/stylesheet/docbook-xsl/html/docbook.xsl \
  opcgw-user-manual.xml
```

#### HTML (Chunked)
```bash
xsltproc --output index.html \
  /usr/share/xml/docbook/stylesheet/docbook-xsl/html/chunk.xsl \
  opcgw-user-manual.xml
```

#### PDF
```bash
fop -xml opcgw-user-manual.xml \
  -xsl /usr/share/xml/docbook/stylesheet/docbook-xsl/fo/docbook.xsl \
  -pdf opcgw-user-manual.pdf
```

#### EPUB (E-book)
```bash
pandoc --from docbook --to epub \
  --output opcgw-user-manual.epub \
  opcgw-user-manual.xml
```

## Manual Sections

### Part 1: Introduction (Chapters 1-2)
- Chapter 1: Overview, key features, system architecture, data flow
- Chapter 2: System requirements (hardware, software, network)

### Part 2: Installation and Setup (Chapters 3-5)
- Chapter 3: Installation via Docker or source
- Chapter 4: Configuration guide ([global], [logging], [chirpstack],
  [opcua], [command_delivery], [[application]] hierarchy)
- Chapter 5: Command-line flags, startup verification, graceful shutdown

### Part 3: Operation and Observability (Chapters 6-8)
- Chapter 6: Daily operation — Gateway folder in OPC UA, stale-data
  status codes, KPIs, command queue monitoring
- Chapter 7: Logging and observability — five-appender architecture,
  request_id correlation, performance budgets, common operations table
- Chapter 8: Troubleshooting symptom cookbook —
  ChirpStack outage, error spike, SQLITE_BUSY, budget exceedance,
  no-data-yet, OPC UA connect failures, stale metrics, DB integrity

### Part 4: Maintenance and Reference (Chapter 9 + Appendices)
- Chapter 9: Database maintenance, historical metric pruning,
  command history retention, log rotation
- Appendix A: Configuration reference table (every TOML key + override)
- Appendix B: OPC UA address space layout
- Appendix C: Operation names — pointer to docs/logging.md
- Appendix D: Glossary

## Editing the Manual

The manual is structured to be maintainable and modular:

1. **To update content:** Edit the appropriate section in `opcgw-user-manual.xml`
2. **To add new sections:** Add new `<chapter>` or `<section>` elements following DocBook structure
3. **To add cross-references:** Use `<xref linkend="id">` to reference other sections
4. **To add external links:** Use `<link xlink:href="url">text</link>`
5. **To add code examples:** Use `<programlisting language="language">code</programlisting>`

## DocBook Resources

- **DocBook 5.2 Specification:** http://docbook.org/xml/5.2/
- **Style Guide:** https://tdg.docbook.org/
- **XSL Stylesheets:** https://github.com/docbook/xslt10-stylesheets

## Tools

### Required for Building
- **xsltproc** — XML stylesheet processor (part of libxslt)
- **FOP** — XSL-FO processor (for PDF)
- **Pandoc** — Document converter (for EPUB)

### Installation
**Ubuntu/Debian:**
```bash
sudo apt-get install libxslt1-tools fop pandoc
```

**macOS:**
```bash
brew install libxslt fop pandoc
```

## Validation

Validate the XML against DocBook schema:
```bash
xmllint --schema /usr/share/xml/docbook/xml5.2/xsd/docbook.xsd \
  --noout opcgw-user-manual.xml
```

## Version History

- **v2.0** (2026-04-27) — Comprehensive update covering Epics 2-6
  - Configuration chapter rewritten against the actual TOML schema
    (the v1.0 manual documented placeholder keys that did not match
    the implementation).
  - Added Epic 2 deliverables: SQLite WAL persistence, per-task
    connection pool, batched writes, historical pruning.
  - Added Epic 3 deliverables: SQLite-backed FIFO command queue with
    parameter validation and three-state delivery tracking.
  - Added Epic 4 deliverables: gRPC pagination via `list_page_size`,
    multi-type metric support (Bool/Int/Float/String), counter/gauge/
    absolute kind handling.
  - Added Epic 5 deliverables: Gateway folder in OPC UA
    (`LastPollTimestamp`, `error_count`, `chirpstack_available`),
    stale-data status codes (Good/Uncertain/Bad).
  - Added Epic 6 deliverables: structured tracing with microsecond
    timestamps, configurable verbosity (CLI/env/TOML precedence chain),
    per-module log files, `request_id` correlation, performance-budget
    warnings, full operation-name taxonomy.
  - New Chapter 7 (Logging & Observability) and rewritten Chapter 8
    (Troubleshooting) with a symptom cookbook.
  - New appendices: configuration reference table, OPC UA address
    space layout, operation names.

- **v1.0** (2026-04-20) — Initial release covering essential topics for
  opcgw v1.0 (configuration keys partly placeholder; superseded by v2.0).

## License

This documentation is licensed under MIT OR Apache-2.0, same as the opcgw project.

## Contact

For documentation issues or suggestions:
- Open an issue on GitHub: https://github.com/guycorbaz/opcgw/issues
- Contact: Guy Corbaz (gcorbaz@gmail.com)
