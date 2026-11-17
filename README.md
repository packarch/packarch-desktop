# packarch-rwdesktop

Compositeur Wayland à fenêtres flottantes pour Packarch (programme
`packarch`, configuration dans `~/.config/packarch/`), écrit en Rust sur
[Smithay](https://github.com/Smithay/smithay) : fenêtres et menus aux coins
arrondis et ombrés, thèmes et configuration au format Openbox (`rc.xml`,
`menu.xml`, `themerc`), animations sobres.

État : **prototype 0.1** (voir [docs/cahier-des-charges.md](docs/cahier-des-charges.md),
§ 14). Fenêtres xdg-shell décorées par le compositeur (thème Openbox, coins
arrondis, ombres), couches layer-shell (waybar, swaybg), déplacement,
redimensionnement, maximisation, fermeture. Pas encore de raccourcis, de
menus ni de session réelle (backend udev).

## Essayer

```sh
cargo build
# Dans une fenêtre de la session en cours, avec la config de labwc :
./target/debug/packarch -C ~/.config/labwc -s waybar -s kitty
# Sans écran, captures d'un scénario d'essai dans target/captures :
tests/capture.sh
```

Options cachées pour les essais : `--backend headless`, `--size 1920x1200`,
`--scale 1.5`, `--script FICHIER` (format décrit dans `src/script.rs`).

## Licence

Au choix :

- licence Apache, version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) ;
- licence MIT ([LICENSE-MIT](LICENSE-MIT)).

Sauf mention contraire explicite, toute contribution soumise pour être
incluse dans ce projet, au sens de la licence Apache-2.0, est sous ces deux
licences, sans condition supplémentaire.

Aucun code ne provient d'un autre compositeur : les règles de provenance
sont au § 1 du cahier des charges.
