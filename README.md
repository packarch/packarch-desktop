# packarch-desktop

Compositeur Wayland à fenêtres flottantes pour Packarch (programme
`packarch`, configuration dans `~/.config/packarch/`), écrit en Rust sur
[Smithay](https://github.com/Smithay/smithay) : fenêtres et menus aux coins
arrondis et ombrés, thèmes et configuration au format Openbox (`rc.xml`,
`menu.xml`, `themerc`), animations sobres.

État : **version 1.4.5** (voir [docs/cahier-des-charges.md](docs/cahier-des-charges.md),
§ 14, et la page de manuel [docs/packarch.1](docs/packarch.1) :
`man -l docs/packarch.1`).

- Fenêtres décorées par le compositeur (thèmes Openbox, coins arrondis,
  ombres), couches layer-shell, XWayland (option `xwaylandSharp` pour des
  applications X11 nettes à l'échelle 1,25 ou 1,5).
- `rc.xml` complet : raccourcis (AZERTY), souris, bureaux, collage (moitiés et
  quarts d'écran), régions,
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
- Loupe (`ZoomIn`, `ZoomOut`, `ToggleMagnify`) : rectangle qui suit le
  pointeur ou tout l'écran.

Pas encore : XWayland lancé à la première connexion X11 (impossible avec
Smithay 0.7, voir le cahier des charges, § 15).

## Essayer

```sh
cargo build
# Dans une fenêtre de la session en cours :
./target/debug/packarch -C ~/.config/packarch -s waybar -s kitty
# Sur une console libre (Ctrl+Alt+F3), en session complète :
./target/debug/packarch -C ~/.config/packarch
# Sans écran, scénarios d'essai (tests/scenarios), captures dans target/captures :
tests/capture.sh
# Captures comparées aux images de référence (tests/references) :
tests/capture.sh tests/scenarios/reference*.txt
# Après un changement voulu du rendu, refaire les références :
tests/capture.sh --update tests/scenarios/reference*.txt
```

Options cachées pour les essais : `--backend headless`, `--size 1920x1200`,
`--scale 1.5`, `--script FICHIER` (format décrit dans `src/script.rs`, avec
une horloge simulée pour capturer une animation à un instant donné).

## Installer hors de Packarch

`packarch` est un compositeur autonome, comme Openbox ou labwc : il marche
sans le reste du bureau Packarch (barre, menus rofi, thèmes). Il faut
Rust 1.88 ou plus récent (rustup si celui de la distribution est plus
ancien) et les bibliothèques de développement de libdrm, libinput,
libxkbcommon, libseat (logind ou seatd), libudev, Mesa (EGL, GBM) ; Xwayland
pour les applications X11.

```sh
cargo build --release
sudo install -Dm755 target/release/packarch /usr/local/bin/packarch
sudo install -Dm755 target/release/packarch-partage /usr/local/bin/packarch-partage
sudo install -Dm644 data/rc.xml /etc/xdg/packarch/rc.xml
sudo install -Dm644 data/menu.xml /etc/xdg/packarch/menu.xml
sudo install -Dm644 data/packarch.desktop /usr/share/wayland-sessions/packarch.desktop
sudo install -Dm644 data/packarch-session.target /usr/lib/systemd/user/packarch-session.target
sudo install -Dm644 data/packarch-portals.conf /usr/share/xdg-desktop-portal/packarch-portals.conf
sudo install -Dm644 data/xdg-desktop-portal-wlr.conf /etc/xdg/xdg-desktop-portal-wlr/packarch
sudo install -Dm644 docs/packarch.1 /usr/local/share/man/man1/packarch.1
```

La session « packarch » apparaît alors dans le gestionnaire de connexion
(ou `packarch` depuis une console). Sans `~/.config/packarch/rc.xml`, la
configuration de `/etc/xdg/packarch` s'applique : clic droit sur le bureau
ou Super+Espace pour le menu (applications installées par catégorie,
fenêtres ouvertes, terminal, rechargement, veille, redémarrage, arrêt), Super+Entrée pour un terminal (`$TERMINAL`,
sinon foot, kitty, alacritty… le premier installé), Super+Maj+e pour
quitter. Pour la modifier, la copier dans `~/.config/packarch/`. Les
thèmes de fenêtres sont ceux d'Openbox (`/usr/share/themes/*/openbox-3`).
Le clavier suit `XKB_DEFAULT_LAYOUT`, sinon la disposition du système
(`localectl`, ou `/etc/default/keyboard` sous Debian et Ubuntu).

## Licence

Au choix :

- licence Apache, version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) ;
- licence MIT ([LICENSE-MIT](LICENSE-MIT)).

Sauf mention contraire explicite, toute contribution soumise pour être
incluse dans ce projet, au sens de la licence Apache-2.0, est sous ces deux
licences, sans condition supplémentaire.

Aucun code ne provient d'un autre compositeur : les règles de provenance
sont au § 1 du cahier des charges.
