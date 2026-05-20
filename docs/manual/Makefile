# opcgw User Manual — DocBook 4.5 → HTML / PDF build pipeline.
#
# Story B-1 D2 (2026-05-19): adds CLI/CI-buildable manual output via xsltproc + dblatex.
# Authors continue to use the oXygen project (opcgw.xpr) for editing; this Makefile
# is the headless alternative for releases, CI, and contributors without oXygen.
#
# Prerequisites (Debian/Ubuntu):
#   sudo apt-get install -y xsltproc docbook-xsl dblatex
# Alternative (PDF via FOP instead of dblatex):
#   sudo apt-get install -y xsltproc docbook-xsl fop docbook-xsl-ns
#
# Usage:
#   make html      # → out/html/index.html (chunked HTML, multi-page)
#   make html-single # → out/opcgw-user-manual.html (single-page HTML)
#   make pdf       # → out/opcgw-user-manual.pdf
#   make all       # html + html-single + pdf
#   make validate  # DocBook 4.5 DTD validation only (no output produced)
#   make clean     # remove out/

MANUAL    := opcgw-user-manual.xml
OUT_DIR   := out
HTML_DIR  := $(OUT_DIR)/html
# Single-page HTML lives in its own subdir so the logo's `../logo/…` fileref
# resolves to `$(OUT_DIR)/logo/` (parent of the HTML file), matching the
# chunked output's resolution path.
HTML_SINGLE_DIR := $(OUT_DIR)/html-single
HTML_SINGLE     := $(HTML_SINGLE_DIR)/opcgw-user-manual.html
PDF_OUT   := $(OUT_DIR)/opcgw-user-manual.pdf

# Standard Debian/Ubuntu install path for DocBook XSL stylesheets.
# Override on the command line if your distro installs to a different prefix:
#   make html DOCBOOK_XSL_DIR=/usr/local/share/xml/docbook/stylesheet/nwalsh
DOCBOOK_XSL_DIR ?= /usr/share/xml/docbook/stylesheet/docbook-xsl

# Logos referenced from the manual via fileref="../logo/opcgw-horizontal.svg".
# xsltproc needs the original SVGs available relative to OUT_DIR for the HTML
# build. Declared as explicit prerequisites so `make` fails fast if any logo
# is missing — before xsltproc writes partial output. Per-file targets also
# prevent the `make -j` race that two unconditional `cp` recipes would create.
LOGO_SRC   := ../logo
LOGO_FILES := opcgw-horizontal.svg opcgw-mark.svg opcgw-favicon.svg
LOGO_DEST  := $(addprefix $(OUT_DIR)/logo/,$(LOGO_FILES))

.PHONY: all html html-single pdf validate clean print-deps

all: html html-single pdf

# ── HTML (chunked, multi-page) ─────────────────────────────────────────────
html: $(MANUAL) $(LOGO_DEST) | $(HTML_DIR)
	@echo "→ Building chunked HTML to $(HTML_DIR)/"
	xsltproc \
	    --output $(HTML_DIR)/ \
	    --xinclude \
	    --stringparam chunker.output.encoding UTF-8 \
	    --stringparam use.id.as.filename 1 \
	    $(DOCBOOK_XSL_DIR)/html/chunk.xsl \
	    $(MANUAL)
	@echo "✓ HTML manual:  $(HTML_DIR)/index.html"

# ── HTML (single-page) ─────────────────────────────────────────────────────
html-single: $(MANUAL) $(LOGO_DEST) | $(HTML_SINGLE_DIR)
	@echo "→ Building single-page HTML to $(HTML_SINGLE)"
	xsltproc \
	    --output $(HTML_SINGLE) \
	    --xinclude \
	    $(DOCBOOK_XSL_DIR)/html/docbook.xsl \
	    $(MANUAL)
	@echo "✓ Single-page:  $(HTML_SINGLE)"

# ── PDF (via dblatex; preferred for SVG support) ───────────────────────────
pdf: $(MANUAL) | $(OUT_DIR)
	@echo "→ Building PDF to $(PDF_OUT) (dblatex)"
	dblatex -t pdf -o $(PDF_OUT) $(MANUAL)
	@echo "✓ PDF manual:   $(PDF_OUT)"

# ── DocBook 4.5 DTD validation ─────────────────────────────────────────────
validate: $(MANUAL)
	@echo "→ Validating $(MANUAL) against DocBook 4.5 DTD"
	xmllint --noout --valid $(MANUAL)
	@echo "✓ Valid"

# ── Print build-tool versions (for CI logs / debugging) ────────────────────
print-deps:
	@echo "xsltproc:  $$(xsltproc --version 2>&1 | head -1 || echo MISSING)"
	@echo "xmllint:   $$(xmllint --version 2>&1 | head -1 || echo MISSING)"
	@echo "dblatex:   $$(dblatex --version 2>&1 | head -1 || echo MISSING)"
	@echo "fop:       $$(fop -version 2>&1 | head -1 || echo MISSING)"
	@echo "DocBook XSL dir: $(DOCBOOK_XSL_DIR) ($$(test -d $(DOCBOOK_XSL_DIR) && echo present || echo MISSING))"

clean:
	rm -rf $(OUT_DIR)

# ── Directories ────────────────────────────────────────────────────────────
$(OUT_DIR):
	@mkdir -p $(OUT_DIR)

$(HTML_DIR): | $(OUT_DIR)
	@mkdir -p $(HTML_DIR)

$(HTML_SINGLE_DIR): | $(OUT_DIR)
	@mkdir -p $(HTML_SINGLE_DIR)

$(OUT_DIR)/logo: | $(OUT_DIR)
	@mkdir -p $(OUT_DIR)/logo

# Pattern rule: each per-file logo copy depends on its source SVG.
# A missing source SVG triggers a clean make-level failure before any
# xsltproc run, instead of leaving partial $(HTML_DIR)/ output.
$(OUT_DIR)/logo/%.svg: $(LOGO_SRC)/%.svg | $(OUT_DIR)/logo
	cp $< $@
