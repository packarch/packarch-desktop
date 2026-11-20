# À faire

Relevé du 19 novembre 2026 (écarts au cahier des charges et améliorations).

## Fait

- [x] Journal des paniques (§ 13) — b65d2ef
- [x] Script de vérification `tests/ci.sh` (§ 13) — af25889
- [x] Captures headless déterministes (rendu complet) — ff5cc2c
- [x] Pointeur virtuel, wlr-virtual-pointer v2 (décision 25) — 0629b98
- [x] wlr-screencopy : dmabuf, dégâts réels, curseur au choix (décision 26) — bf15650
- [x] ext-image-copy-capture et ext-image-capture-source, capture d'une
  fenêtre (décision 27) — 260ebb4
- [x] Rythme des clients sur deux écrans, headless à plusieurs écrans — da125c7
- [x] Tests sans GPU par Mesa llvmpipe au lieu de pixman (décision 28) — 8daa02a

## Examen du code (20 novembre 2026, packarch 1.4.9)

Relevé à la lecture du code, sans essai en session : à reproduire avant de
corriger. Non relus en détail : `render.rs`, `menu/`, `config.rs`, le patch
`vendor/smithay`.

### Corrections

- [x] **`Kill` sur une fenêtre X11 tuait XWayland entier** : `kill`
  (`src/wm.rs`) prend maintenant le processus de `_NET_WM_PID`
  (`X11Surface::pid()`) ; sans lui, la fenêtre est seulement fermée.
  Non essayé avec une application X11.
- [x] **Échelle fausse avec deux écrans d'échelles différentes** :
  corrigé par `update_surface_outputs` (`src/handlers.rs`), appelé après
  chaque rendu et chaque changement d'écran : écran du centre du cadre,
  `wl_surface.enter`/`leave` par écran touché. Scénario `echelle-ecrans`
  (commandes `output-scale` et `expect-scale`). Non essayé sur deux vrais
  écrans. Avant :
  - `surface_scale` (`src/handlers.rs`) donne à toute surface l'échelle du
    premier écran ;
  - `output_changed` (`src/protocols/outputs.rs`) applique l'échelle de
    l'écran modifié à toutes les vues, où qu'elles soient ;
  - `map_view` (`src/handlers.rs`) annonce `output_enter` sur le premier
    écran seulement, jamais mis à jour quand la vue change d'écran.
- [ ] **XWayland n'est pas relancé s'il plante** : `disconnected`
  (`src/xwayland.rs`) vide l'état, mais `DISPLAY` reste exporté ; les
  applications X11 échouent jusqu'à la fin de la session. Vérifier aussi
  que les vues X11 restantes sont retirées.
- [x] **`<libinput>` n'était pas réappliqué au rechargement** (§ 5.7) :
  les périphériques branchés sont retenus, `reconfigure` appelle
  `configure_devices` (`src/backend/udev.rs`). Un réglage retiré de
  `rc.xml` garde sa valeur jusqu'au prochain branchement. Non essayé en
  session réelle.
- [x] **`-s COMMANDE` partait trop tôt** (avant `autostart` et XWayland,
  sans `DISPLAY`) : lancée par `run_autostart` (`src/session.rs`), après
  le script `autostart` (§ 12).
- [x] **Fin de session incomplète** :
  - `XDG_SESSION_TYPE` est retiré de l'environnement de systemd (même
    liste `EXPORTED` pour l'export et le retrait, `src/session.rs`) ;
  - une panique dans la boucle d'événements passe par `end_session` avant
    de sortir (`run`, `src/main.rs`). Non essayé.
- [ ] **Écran débranché : couches non fermées** : `udev_scan`
  (`src/backend/udev.rs`) fait seulement `cleanup()`, sans `send_close` aux
  surfaces de l'écran retiré (barre waybar de cet écran) ; voir aussi les
  surfaces de verrouillage. À confirmer en session réelle.
- [ ] **Configuration d'écrans appliquée à moitié** :
  `apply_output_config` (`src/protocols/outputs.rs`) rend `false` au
  premier `set_mode` en échec, sans annuler les écrans déjà changés.

### Oublis

- [ ] **Écrans tactiles et tablettes** : événements ignorés (`_ => {}` de
  `process_input`, `src/input.rs`), siège sans `wl_touch`.
- [ ] **Répétition des raccourcis** : une touche maintenue (volume,
  `MoveRelative`, `ResizeRelative`) ne déclenche l'action qu'une fois.
- [ ] **wlr-gamma-control** : gammastep et wlsunset (lumière de nuit) ne
  peuvent pas marcher ; à ajouter au § 9.2 du cahier des charges.
- [ ] **`show_window_menu` (xdg-shell)** : le clic droit sur la barre de
  titre d'une application à GtkHeaderBar n'ouvre pas le menu de fenêtre.
- [ ] **`fullscreen_request` ignore l'écran demandé**
  (`src/handlers.rs`).
- [ ] **wlr-output-management** : rotation (`transform`) et désactivation
  d'un écran refusées (kanshi avec `disable` ou `transform` échoue).
- [ ] **`--version` sans le commit git**, annoncé au § 12.

### Qualité et outillage

- [ ] **`cargo deny` ne tourne jamais** : absent de la machine,
  `tests/ci.sh` saute l'étape en silence ; licences (§ 1.2) et avis de
  sécurité non vérifiés. Installer `cargo-deny` et faire échouer l'étape
  quand il manque.
- [ ] **Pas de CI sur GitHub** (pas de `.github`) : `tests/ci.sh
  --logiciel` est fait pour tourner sans GPU.
- [ ] **PKGBUILD** (`myrepo-pkgs/apps/packarch-desktop`) :
  `sha256sums=('SKIP')` sur l'archive du tag.
- [ ] **Fond par défaut** : image rendue et gardée en mémoire même quand
  swaybg la cache (`output_elements`, `src/render.rs`) ; cache par taille
  de `src/logo.rs` jamais vidé. À mesurer face à l'objectif de 60 Mio
  (§ 2.1).

## Bloqué par Smithay (décision 29)

À revoir à la prochaine version publiée de Smithay (0.7.0 au 19 novembre
2026, branche principale sans changement sur ces points) :

- [ ] tearing-control (§ 9.2) : demande le page flip asynchrone
  (`DRM_MODE_PAGE_FLIP_ASYNC`) dans `DrmCompositor`.
- [ ] Point chaud du plan curseur : retirer le patch `vendor/smithay`
  (décision 18).
- [ ] XWayland à la première connexion X11 (décision 12).
- [ ] xdg-toplevel-icon (décision 11).

## À faire : sélecteur de fenêtres pour le partage

xdg-desktop-portal-wlr 0.8 sait partager une seule fenêtre, mais son
sélecteur doit renvoyer `Window: <identifiant ext-foreign-toplevel>` :
`slurp` (sélecteur par défaut) ne propose que des écrans, `rofi` en mode
dmenu ne reçoit que la liste des écrans, et `lswt` n'est pas dans les
dépôts.

- [x] `packarch-partage` : liste écrans et fenêtres dans rofi, rend
  « Monitor: NOM » ou « Window: IDENTIFIANT » (décision 30) — ecb8893
- [x] Config du portail `data/xdg-desktop-portal-wlr.conf`
  (`chooser_type=simple`, `chooser_cmd=packarch-partage`) — ecb8893
- [x] Paquet 1.4.1 : `packarch-partage` dans `/usr/bin`, config dans
  `/etc/xdg/xdg-desktop-portal-wlr/packarch` (`backup=`), `rofi` en
  dépendance optionnelle — publié et installé le 19 novembre 2026.

## Essayé en session réelle (packarch 1.4.2, 19 novembre 2026)

- [x] **Partage d'écran dans Firefox** :
  `https://mozilla.github.io/webrtc-landing/gum_test.html`, bouton
  « Screen », choisir un écran dans le sélecteur rofi de
  `packarch-partage` ; l'image suit l'écran.
- [x] **Partage d'une fenêtre dans Firefox** : même page, ligne
  « Fenêtre » ; elle reste entière même recouverte
  (ext-image-copy-capture).
- [x] **Partage d'écran par le portail sur la NVIDIA** : les dmabuf
  linéaires de xdg-desktop-portal-wlr passent par la copie processeur
  (décision 31) ; les deux essais ci-dessus fonctionnent.
