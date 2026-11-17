# packarch-rwdesktop

Compositeur Wayland à fenêtres flottantes pour Packarch (programme
`packarch`, configuration dans `~/.config/packarch/`), écrit en Rust sur
[Smithay](https://github.com/Smithay/smithay) : fenêtres et menus aux coins
arrondis et ombrés, thèmes et configuration au format Openbox (`rc.xml`,
`menu.xml`, `themerc`), animations sobres.

État : **version 0.3** (voir [docs/cahier-des-charges.md](docs/cahier-des-charges.md),
§ 14). Fenêtres décorées par le compositeur (thème Openbox, coins arrondis,
ombres), couches layer-shell (waybar, swaybg), `rc.xml` complet pour les
réglages de Packarch : raccourcis (y compris en AZERTY), liaisons de la
souris, bureaux, collage aux bords et aux régions, règles de fenêtres,
`environment`, `autostart`, rechargement (`packarch --reconfigure` ou
Ctrl+Maj+R). Menus de `menu.xml` arrondis et ombrés : sous-menus au survol,
pipemenus asynchrones, icônes (png, svg, thème d'icônes), menu de fenêtre,
liste des fenêtres, navigation au clavier. Animations : menus, ouverture,
fermeture, réduction et restauration des fenêtres (réglables dans
`<animations>`, `--no-animations`). Pas encore de session réelle
(backend udev, 0.4) ; les animations de maximisation, d'enroulement, de
changement de bureau et des OSD restent à faire.

## Essayer

```sh
cargo build
# Dans une fenêtre de la session en cours, avec la config de labwc :
./target/debug/packarch -C ~/.config/labwc -s waybar -s kitty
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
