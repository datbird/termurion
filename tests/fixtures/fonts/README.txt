TermurionWideBoldTest-Regular.ttf and -Bold.ttf: a test family whose bold member is 10 percent
wider than the regular one (render::tests::a_wider_bold_face_stays_on_the_cell_grid). Built from
DejaVu Sans Mono and DejaVu Sans Mono Bold, ASCII only, renamed, with make.py (fontTools):
  python3 make.py tests/fixtures/fonts
DejaVu fonts license: LICENSE-DejaVu in this folder.
TermurionLigatureTest-Regular.ttf: DejaVu Sans Mono with "=>" and "===" turned into one arrow
glyph one cell wide by a liga feature, and "é" 1.5 cells wide (render::tests::a_ligature_advances_one_cell_per_character and others).
TermurionRtlTest-Regular.ttf: DejaVu Sans with its Hebrew and Arabic letters and shaping features,
renamed (render::tests right-to-left and joined-run tests).
