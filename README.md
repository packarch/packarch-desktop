# packarch-rwdesktop

Compositeur Wayland à fenêtres flottantes pour Packarch (programme
`packarch`, configuration dans `~/.config/packarch/`), écrit en Rust sur
[Smithay](https://github.com/Smithay/smithay) : fenêtres et menus aux coins
arrondis et ombrés, thèmes et configuration au format Openbox (`rc.xml`,
`menu.xml`, `themerc`), animations sobres.

État : **version 0.5** (voir [docs/cahier-des-charges.md](docs/cahier-des-charges.md),
§ 14). Fenêtres décorées par le compositeur (thème Openbox, coins arrondis,
ombres), couches layer-shell, `rc.xml` complet pour les réglages de Packarch
(raccourcis en AZERTY, souris, bureaux, collage, régions, règles de
fenêtres, rechargement), menus de `menu.xml` arrondis et ombrés avec
pipemenus, animations.

Session réelle (backend udev : DRM/KMS, libseat, libinput, plusieurs
écrans, branchement à chaud, Ctrl+Alt+F1…F12) ; liste Alt+Tab et nom du
bureau à l'écran ; protocoles du quotidien : verrouillage (hyprlock),
inactivité (swayidle), barre des tâches et bureaux de waybar, capture
(grim), wlr-randr, wlopm, presse-papiers des outils, activation, méthodes
de saisie, contraintes du pointeur, bacs à sable (security-context).

XWayland : applications X11 décorées comme les autres, menus et bulles X11,
focus clavier X, presse-papiers partagé, `_NET_WM_STATE`, dialogues centrés
sur leur parent. Fichiers d'intégration (session, portails) dans
[data/](data/).

Pas encore : animations de maximisation, d'enroulement et de collage ;
priorité 2 des actions (1.0).

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
