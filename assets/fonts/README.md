# Bundled fonts

Built into the binary. Source Code Pro and Iosevka are the symbol list's
font choices (File > Settings…), under the SIL Open Font License 1.1.
Ubuntu Regular draws the app's text in place of egui's thinner Ubuntu
Light, and Ubuntu Medium the pane toggles in the menu bar, under the
Ubuntu Font Licence 1.0. Each license is next to its font and the
copyright notices are kept in the fonts' name tables.

| File | Font | Source | Changes |
|------|------|--------|---------|
| `SourceCodePro-Regular.ttf` | Source Code Pro 2.042 | [adobe-fonts/source-code-pro](https://github.com/adobe-fonts/source-code-pro) release `2.042R-u/1.062R-i/1.026R-vf`, `TTF-source-code-pro-2.042R-u_1.062R-i.zip` | None. "Source" is a Reserved Font Name, so the file is shipped as released. |
| `Ubuntu-Regular.ttf`, `Ubuntu-Medium.ttf` | Ubuntu 0.83 (the version of the Ubuntu Light egui ships) | [google/fonts](https://github.com/google/fonts/tree/main/ufl/ubuntu) `ufl/ubuntu`, commit `a50de97` | None. Licence: `LICENSE-Ubuntu.txt`. |
| `Iosevka-Regular.ttf` | Iosevka 34.9.0 | [be5invis/Iosevka](https://github.com/be5invis/Iosevka) release `v34.9.0`, `PkgTTF-Unhinted-Iosevka-34.9.0.zip` | Subset (8.4 MB → 116 KB): Latin, Latin-1, Latin Extended-A, punctuation, arrows, math, box drawing, shapes and dingbats; no hinting or OpenType features. Iosevka has no Reserved Font Name. |

To rebuild the Iosevka subset from the release's `Iosevka-Regular.ttf`:

```sh
uvx --from 'fonttools==4.55.0' pyftsubset Iosevka-Regular.ttf \
  --unicodes='U+0020-007E,U+00A0-017F,U+2010-205E,U+2190-21FF,U+2200-22FF,U+2500-25FF,U+2600-26FF,U+2700-27BF' \
  --layout-features='' --no-hinting --desubroutinize \
  --name-IDs='*' --name-languages='*' --name-legacy \
  --output-file=Iosevka-Regular.ttf.subset
```
