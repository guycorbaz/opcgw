# opcgw — Logo pack

Pack de logos officiels pour **opcgw**, passerelle ChirpStack vers OPC UA écrite en Rust.

Repo : https://github.com/guycorbaz/opcgw

---

## Concept

Le monogramme combine deux références visuelles :

- **L'hexagone bleu marine** évoque le nœud industriel / SCADA et le protocole OPC UA, ainsi que la structure modulaire des crates Rust.
- **Le bord orange Rust** rappelle le langage d'implémentation.
- **Les lettres `o` et `g` géométrisées** au centre forment le monogramme du nom du projet (opcgw).

---

## Fichiers inclus

| Fichier | Dimensions | Usage |
|---|---|---|
| `opcgw-mark.svg` | 200 × 200 | Icône seule. Avatar GitHub, badge crate.io, app icon, social preview |
| `opcgw-horizontal.svg` | 520 × 100 | Mark + wordmark + baseline. Header de README, site web, signature email |
| `opcgw-favicon.svg` | 32 × 32 | Version optimisée pour les très petites tailles (strokes ajustés) |

Tous les fichiers sont en SVG vectoriel : redimensionnables à l'infini sans perte, et éditables dans n'importe quel éditeur (Inkscape, Illustrator, Figma).

---

## Palette de couleurs

| Couleur | HEX | RGB | Usage |
|---|---|---|---|
| Bleu industriel | `#1A3F5C` | 26, 63, 92 | Couleur principale, fond hexagone, wordmark |
| Orange Rust | `#CE7B3C` | 206, 123, 60 | Couleur d'accent, bordure, lettres `gw` |
| Blanc | `#FFFFFF` | 255, 255, 255 | Lettre `g`, contraste sur fond bleu |

---

## Intégration dans le README du projet

Ajoute en haut de ton `README.md` :

```markdown
<p align="center">
  <img src="assets/opcgw-horizontal.svg" alt="opcgw" width="400">
</p>
```

Pour un badge plus compact (à côté du titre) :

```markdown
# <img src="assets/opcgw-mark.svg" width="32" align="left"> opcgw
```

---

## Conversion vers d'autres formats

### En PNG (pour social preview GitHub, README qui ne supporte pas SVG, etc.)

Avec **ImageMagick** :

```bash
convert -background none -resize 1280x640 opcgw-mark.svg opcgw-social.png
convert -background none -resize 512x512 opcgw-mark.svg opcgw-512.png
convert -background none -resize 256x256 opcgw-mark.svg opcgw-256.png
```

Avec **rsvg-convert** (plus fidèle pour les SVG) :

```bash
rsvg-convert -w 512 -h 512 opcgw-mark.svg -o opcgw-512.png
```

Avec **Inkscape** en ligne de commande :

```bash
inkscape opcgw-mark.svg --export-type=png --export-width=512 --export-filename=opcgw-512.png
```

### En ICO (favicon multi-résolution Windows)

```bash
convert opcgw-favicon.svg -define icon:auto-resize=16,32,48,64 favicon.ico
```

---

## Variantes à générer si besoin

Pour les cas particuliers (impression mono, fond foncé, etc.), les variantes suivantes peuvent être dérivées du fichier source :

- **Bleu seul** : remplacer `stroke="#CE7B3C"` par `stroke="#1A3F5C"` et le cercle orange par du blanc
- **Orange seul** : remplacer `fill="#1A3F5C"` par `fill="#CE7B3C"`
- **Outline (impression noir et blanc)** : remplacer `fill="#1A3F5C"` par `fill="none"` et toutes les couleurs par `#000000`
- **Pour fond sombre** : remplacer `fill="#1A3F5C"` par `fill="#FFFFFF"` et ajuster les contrastes

---

## Licence

Logo créé pour le projet opcgw. À aligner avec la licence du projet principal (voir le repo).

---

*Pack généré le 19 mai 2026.*
