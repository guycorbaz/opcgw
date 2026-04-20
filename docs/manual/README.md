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
- Chapter 1: Overview of opcgw, architecture, key features
- Chapter 2: System requirements (hardware, software, network)

### Part 2: Installation and Setup (Chapters 3-5)
- Chapter 3: Installation via Docker or source
- Chapter 4: Configuration guide (ChirpStack, OPC UA, logging)
- Chapter 5: Startup verification and graceful shutdown

### Part 3: Operation and Monitoring (Chapters 6-8)
- Chapter 6: Daily operation and health checks
- Chapter 7: Troubleshooting guide with common issues
- Chapter 8: Maintenance procedures (database, pruning, logs)

### Part 4: Appendices
- Complete configuration reference
- Error codes and messages
- Glossary of terms

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

- **v1.0** (2026-04-20) — Initial release covering all essential topics for opcgw v1.0
  - Installation procedures (Docker and source)
  - Complete configuration reference
  - Operation and troubleshooting guides
  - Maintenance procedures
  - API reference and examples

## License

This documentation is licensed under MIT OR Apache-2.0, same as the opcgw project.

## Contact

For documentation issues or suggestions:
- Open an issue on GitHub: https://github.com/guycorbaz/opcgw/issues
- Contact: Guy Corbaz (gcorbaz@gmail.com)
