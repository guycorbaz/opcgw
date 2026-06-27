# opcgw User Manual — LaTeX (LuaLaTeX) build.
#
# LaTeX is the canonical manual format (DocBook retired 2026-06-27, GH #145).
# The hand-maintained sources live in latex/: main.tex, preamble.tex, body.tex.
#
# Prerequisites (Debian/Ubuntu):
#   sudo apt-get install -y texlive-luatex texlive-latex-extra texlive-fonts-extra \
#                           fonts-lato fonts-noto fonts-dejavu graphviz mupdf-tools
#
# Usage:
#   make pdf         # → latex/opcgw-user-manual.pdf
#   make clean       # remove generated LaTeX artifacts
#   make print-deps  # show build-tool versions

.PHONY: pdf clean print-deps

pdf:
	@echo "→ Building modern LaTeX PDF (latex/build.sh)"
	cd latex && ./build.sh
	@echo "✓ PDF manual: latex/opcgw-user-manual.pdf"

clean:
	@echo "→ Cleaning LaTeX build artifacts"
	cd latex && latexmk -C main.tex >/dev/null 2>&1 || true
	rm -rf latex/figures latex/version.tex latex/opcgw-user-manual.pdf
	@echo "✓ Clean"

print-deps:
	@echo "lualatex: $$(lualatex --version 2>&1 | head -1 || echo MISSING)"
	@echo "latexmk:  $$(latexmk --version 2>&1 | head -1 || echo MISSING)"
	@echo "dot:      $$(dot -V 2>&1 | head -1 || echo MISSING)"
	@echo "mutool:   $$(mutool -v 2>&1 | head -1 || echo MISSING)"
