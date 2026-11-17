# packarch-rwdesktop

Compositeur Wayland à fenêtres flottantes pour Packarch (programme
`packarch`, configuration dans `~/.config/packarch/`), écrit en Rust sur
[Smithay](https://github.com/Smithay/smithay) : fenêtres et menus aux coins
arrondis et ombrés, thèmes et configuration au format Openbox (`rc.xml`,
`menu.xml`, `themerc`), animations sobres.

État : **version 1.0** (voir [docs/cahier-des-charges.md](docs/cahier-des-charges.md),
§ 14, et la page de manuel [docs/packarch.1](docs/packarch.1) :
`man -l docs/packarch.1`).

- Fenêtres décorées par le compositeur (thèmes Openbox, coins arrondis,
  ombres), couches layer-shell, XWayland.
- `rc.xml` complet : raccourcis (AZERTY), souris, bureaux, collage, régions,
  règles de fenêtres, toutes les actions de priorité 1 et 2 (`If`,
  `ForEach`, `MoveToEdge`, `MoveToOutput`…), rechargement.
- Menus de `menu.xml` arrondis et ombrés, pipemenus, menu de fenêtre, liste
  des fenêtres ; liste Alt+Tab, nom du bureau, taille pendant un
  redimensionnement.
- Animations : fenêtres (ouverture, fermeture, réduction, maximisation,
  collage, enroulement), menus, OSD, bureaux, aperçu du collage.
- Session réelle (DRM/KMS, libseat, libinput, plusieurs écrans) ;
  verrouillage, inactivité, barre des tâches et bureaux de waybar, capture,
  wlr-randr, wlopm, presse-papiers des outils, portails.

Pas encore : loupe (`ZoomIn`/`ZoomOut`, priorité 3), quarts d'écran
(`SnapToEdge combine="yes"`), XWayland lancé à la première connexion X11.

## Essayer

```sh
cargo build
# Dans une fenêtre de la session en cours, avec la config de labwc :
./target/debug/packarch -C ~/.config/labwc -s waybar -s kitty
# Sur une console libre (Ctrl+Alt+F3), en session complète :
./target/debug/packarch -C ~/.config/labwc
# Sans écran, scénarios d'essai (tests/scenarios), captures dans target/captures :
tests/capture.sh
```

Options cachées pour les essais : `--backend headless`, `--size 1920x1200`,
`--scale 1.5`, `--script FICHIER` (format décrit dans `src/script.rs`, avec
une horloge simulée pour capturer une animation à un instant donné).

## Licence

Au choix :

- licence Apache, version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) ;
- licence MIT ([LICENSE-MIT](LICENSE-MIT)).

Sauf mention contraire explicite, toute contribution soumise pour être
incluse dans ce projet, au sens de la licence Apache-2.0, est sous ces deux
licences, sans condition supplémentaire.

Aucun code ne provient d'un autre compositeur : les règles de provenance
sont au § 1 du cahier des charges.
