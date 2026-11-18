# Cahier des charges de packarch-rwdesktop

Version 0.2, 17 novembre 2026. Document de travail, sous MIT ou Apache-2.0
au choix, comme le code.

packarch-rwdesktop (paquet `packarch-rwdesktop`, programme `packarch`) est
un compositeur Wayland à fenêtres flottantes (« stacking »), écrit en Rust sur la boîte à outils Smithay. Il vise le bureau
de Packarch : fenêtres décorées par le compositeur avec coins arrondis et
ombres, menus du bureau arrondis et ombrés, thèmes Openbox, configuration au
format Openbox (`rc.xml`, `menu.xml`), barre et outils Wayland habituels
(waybar, rofi, mako, swaybg, hyprlock, grim…), avec des animations
sobres.

Ce document décrit **ce que le compositeur doit faire**, pas comment un autre
projet le fait. Il sert de référence unique pour écrire le code (voir
§ 1, provenance).

---

## 1. Provenance et licence

### 1.1 Licence

- Code, documentation et ressources du projet : **MIT OR Apache-2.0**, au
  choix de l'utilisateur (`LICENSE-MIT`, `LICENSE-APACHE`). Champ
  `license = "MIT OR Apache-2.0"` dans `Cargo.toml`.
- Toute contribution est réputée faite sous ces deux licences (clause 5 de
  la licence Apache).
- Aucune mention d'un autre compositeur n'est requise, puisqu'aucun code n'en
  provient.

### 1.2 Règles de provenance (« salle blanche »)

1. **Sources autorisées pour écrire le code :**
   - ce cahier des charges ;
   - les spécifications des protocoles Wayland (wayland, wayland-protocols,
     wlr-protocols) et leurs fichiers XML ;
   - la documentation et les exemples de Smithay (MIT), en conservant la
     mention MIT de tout extrait repris ;
   - la documentation des crates utilisées ;
   - la description publique des formats Openbox (`rc.xml`, `menu.xml`,
     `themerc`) et les fichiers de configuration et thèmes de Packarch,
     qui nous appartiennent ;
   - le comportement observé d'un bureau en marche (essais, captures).
2. **Interdit :** copier, traduire ou paraphraser ligne à ligne du code ou de
   la documentation sous GPL ou LGPL, en particulier labwc, Openbox, niri,
   cosmic-comp, Pinnacle, sway et wlroots. Les lire pour comprendre une idée
   générale reste admis ; le code s'écrit ensuite à partir de ce document.
3. **Dépendances :** uniquement des crates sous licence permissive (MIT,
   Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0 au niveau du fichier).
   Vérification par `cargo deny check licenses` en intégration continue,
   avec un `deny.toml` qui refuse GPL, LGPL et AGPL.
   Les bibliothèques système liées dynamiquement (libinput, libseat,
   libxkbcommon, mesa, pixman, fontconfig, freetype) sont sous licences
   permissives (MIT, Expat, FreeType) : aucune contrainte.
4. **Outils externes lancés comme programmes séparés** (Xwayland, waybar,
   rofi…) : pas d'effet sur la licence.
5. Chaque fichier source porte l'en-tête SPDX
   `// SPDX-License-Identifier: MIT OR Apache-2.0`.

### 1.3 Compatibilité et nom

- La compatibilité avec la configuration d'Openbox, et avec les fichiers que
  Packarch écrit aujourd'hui pour son bureau, est un objectif. Le code et les
  textes restent les nôtres.
- Le projet ne se présente pas comme une version d'un autre compositeur.

---

## 2. Objectifs et périmètre

### 2.1 Objectifs

1. **Remplacer le bureau actuel de Packarch** sans que l'utilisateur ne voie
   de différence, à part de meilleurs menus et effets.
2. **Coins arrondis et ombres** pour les fenêtres, les menus, les
   sous-menus et la liste Alt+Tab, réglés par le thème.
3. **Configuration et thèmes Openbox** : les thèmes `OB-Packarch-*` et les
   fichiers écrits par packarch-skin et packarch-labwc-settings fonctionnent
   tels quels (après adaptation des chemins, § 11).
4. **Léger** : démarrage en moins d'une seconde, mémoire au repos inférieure
   à 60 Mio sans client, pas de dépendance à un environnement de bureau.
5. **Robuste** : une erreur de configuration n'empêche jamais le démarrage ;
   un client défaillant ne fait pas tomber le compositeur.

### 2.2 Hors périmètre (au moins jusqu'à la version 1.0)

- Pavage automatique (« tiling »), défilement à la niri.
- Animations spectaculaires (fenêtres « en gelée », cubes, flous animés) :
  seules les animations du § 10.5 sont prévues.
- Panneau, lanceur, notifications, fond d'écran intégrés : ce sont des
  programmes séparés (waybar, rofi, mako, swaybg).
- Session X11. Seul XWayland est prévu, pour les applications X11.
- Bureaux « à la GNOME » (vue d'ensemble, gestes à trois doigts complexes).

---

## 3. Vocabulaire

| Terme | Sens |
|---|---|
| Vue | Une fenêtre de premier niveau d'un client (xdg-toplevel ou X11). |
| Cadre | Ce que le compositeur dessine autour d'une vue : barre de titre, boutons, bordures, ombre. |
| Bureau | Espace de travail virtuel (« desktop » dans `rc.xml`). Il y en a 4 par défaut. |
| Menu | Menu dessiné par le compositeur, décrit par `menu.xml` ou généré (menu de fenêtre, liste des fenêtres). |
| Pipemenu | Menu dont le contenu est la sortie XML d'une commande, exécutée à l'ouverture. |
| Région | Rectangle nommé, en pourcentage de l'écran, où l'on peut placer une vue. |
| Contexte | Zone sous le pointeur (titre, bordure, bouton, client, fond…) qui décide de l'action d'un clic. |

---

## 4. Architecture technique

### 4.1 Pile

| Rôle | Choix | Licence |
|---|---|---|
| Boîte à outils Wayland | `smithay` (fonctionnalités : backend_drm, backend_gbm, backend_libinput, backend_session_libseat, backend_udev, backend_winit, renderer_gl, renderer_pixman, wayland_frontend, xwayland, desktop) | MIT |
| Boucle d'événements | `calloop` (fournie avec Smithay) | MIT |
| Lecture XML | `roxmltree` (lecture seule, rapide) | MIT OR Apache-2.0 |
| Texte (titres, menus) | `cosmic-text` (mise en page, recherche des polices via fontdb) | MIT OR Apache-2.0 |
| Dessin 2D en mémoire | `tiny-skia` | BSD-3-Clause |
| Icônes | `freedesktop-icons` (recherche), `resvg` (SVG), `png` | MIT / Apache-2.0 |
| Journal | `tracing`, `tracing-subscriber` | MIT |
| Ligne de commande | `clap` (dérive) | MIT OR Apache-2.0 |
| Clavier | `xkbcommon` (via Smithay) | MIT |

Version minimale de Rust : la stable du moment, fixée dans
`rust-toolchain.toml`. Édition 2024.

### 4.2 Découpage en modules

```
src/
  main.rs            ligne de commande, journal, choix du backend
  state.rs           état global (Smithay + bureau)
  backend/           udev (DRM/KMS + libseat), winit (fenêtre imbriquée), headless (tests)
  config/            rc.xml, menu.xml, environment, autostart, rechargement
  theme/             themerc, themerc-override, boutons (xbm/png/svg), polices
  render/            éléments de rendu : cadre, ombre, coins, menus, OSD
    shaders/         ombre (gaussienne approchée) et masque de coins arrondis (GLSL)
  shell/             xdg-shell, layer-shell, xwayland, popups, décoration
  view.rs            vue : géométrie, états (maximisée, plein écran, enroulée…)
  desktop/           bureaux, placement, empilement, régions, collage aux bords
  input/             clavier, pointeur, tactile, pavé tactile, raccourcis, souris
  actions.rs         toutes les actions (§ 7), exécutées depuis raccourcis, menus, règles
  menu/              menus dessinés, pipemenus, navigation clavier
  osd/               Alt+Tab, nom du bureau, taille pendant un redimensionnement
  anim/              horloge, courbes, animations en cours (§ 10.5)
  protocols/         protocoles annexes (§ 9)
  ipc.rs             --reconfigure, --exit, signaux
```

### 4.3 Backends

- **udev** (session réelle) : DRM/KMS, plusieurs écrans, libseat (logind ou
  seatd), changement de console virtuelle (Ctrl+Alt+F1…F12), mise en veille et
  reprise sans perdre l'affichage.
- **winit** : le compositeur tourne dans une fenêtre d'une session
  existante, pour le développement.
- **headless + pixman** : sans écran ni GPU, pour les tests automatiques et
  les captures (§ 13).
- Rendu : OpenGL ES 2 (GlesRenderer). Repli pixman si GLES est
  indisponible : ombres et coins dessinés en mémoire par tiny-skia, mis en
  cache.

---

## 5. Fichiers de configuration

### 5.1 Emplacement

- Dossier : `$XDG_CONFIG_HOME/packarch/` (par défaut `~/.config/packarch/`),
  puis `/etc/xdg/packarch/` pour les fichiers absents.
- Ce dossier existe déjà sur les machines Packarch : packarch-hello et
  packarch-pi y rangent leurs réglages dans des sous-dossiers
  (`packarch-hello/`, `packarch-pi/`, `packarch-pi.conf`). Le compositeur ne
  lit que ses propres fichiers (liste ci-dessous) et ses propres
  sous-dossiers (`themes/`, `scripts/`…), et ne touche jamais aux autres.
- `-C, --config-dir DOSSIER` choisit un autre dossier. Il permet d'essayer le
  compositeur sur une copie de `~/.config/labwc`.
- Fichiers lus : `rc.xml`, `menu.xml`, `environment`, `autostart`,
  `shutdown`, `themerc-override`.

### 5.2 `rc.xml`

Format XML à la Openbox. Racine `<packarch_config>`, celle des fichiers
écrits par Packarch. `<openbox_config>` et `<labwc_config>` sont aussi
acceptées, pour lire sans retouche un `rc.xml` existant (le contenu est le
même, seul le nom de la racine change).
Les noms d'éléments et d'attributs ne tiennent pas compte de la casse ;
`yes/no`, `true/false`, `on/off` sont acceptés comme booléens.
Un élément inconnu est ignoré avec un avertissement dans le journal.

Sections et réglages à prendre en charge (ceux utilisés par Packarch sont
marqués **P**, ils sont prioritaires) :

| Section | Réglages |
|---|---|
| `core` | `decoration` (server/client) **P**, `gap` **P**, `xwaylandPersistence` **P**, `xwaylandSharp`, `adaptiveSync`, `reuseOutputMode` |
| `placement` | `policy` (automatic, cursor, center) **P** |
| `theme` | `name` **P**, `icon` **P**, `titlebar/layout` **P**, `titlebar/showTitle` **P**, `cornerRadius` **P**, `keepBorder` **P**, `dropShadows` **P**, `dropShadowsOnTiled` **P**, `font place=` (ActiveWindow, InactiveWindow, MenuHeader, MenuItem, OnScreenDisplay ; `name`, `size`, `weight`, `slant`) **P** |
| `margin` | `top`, `bottom`, `left`, `right` **P** |
| `windowSwitcher` | attributs `preview`, `outlines`, `unshade` ; `osd` (`show`, `style`, `output`) ; `fields/field` (`content` : icon, desktop_entry_name, title… ; `width` en %) **P** |
| `resistance` | `screenEdgeStrength`, `windowEdgeStrength` **P** |
| `resize` | `popupShow` (Never, Always, Nonpixel), `drawContents` **P** |
| `focus` | `followMouse`, `raiseOnFocus` **P** |
| `snapping` | `range` (`inner`, `outer`), `topMaximize` **P**, `cornerRange` |
| `desktops` | `popupTime` (ms), `names/name` (le nombre de noms donne le nombre de bureaux) **P** |
| `regions` | `region` (`name`, `x`, `y`, `width`, `height` en %) **P** |
| `keyboard` | `numlock`, `layoutScope` (global, window), `repeatRate`, `repeatDelay`, `keybind` **P** |
| `mouse` | `doubleClickTime`, `context name=` → `mousebind` **P** |
| `libinput` | `device category=` (default, touchpad, non-touch…) : `tap`, `naturalScroll`, `middleEmulation`, `disableWhileTyping`, `accelProfile`, `pointerSpeed`, `leftHanded`, `scrollMethod` **P** pour les quatre premiers |
| `windowRules` | `windowRule` (§ 8.5) **P** |
| `animations` | `enabled`, `speed`, et par animation `duration` (ms), `curve` (§ 10.5) |
| `magnifier` | `width`, `height` (px ; 0 : tout l'écran), `initScale`, `increment`, `useFilter` (loupe, § 15) |

### 5.3 Raccourcis clavier

- `<keybind key="W-S-Return" allowWhenLocked="yes">` suivi d'une ou
  plusieurs `<action>`.
- Modificateurs : `W` (Super), `A` (Alt), `C` (Ctrl), `S` (Maj), `M` (Mod5 /
  AltGr), `H` (Hyper).
- Touche : nom de keysym xkb (`Return`, `KP_5`, `XF86AudioMute`, `space`,
  `Print`, `Tab`…), ou chiffre/lettre.
- **Clavier AZERTY** : un raccourci écrit avec un chiffre (`W-1`) doit marcher
  sans Maj sur un clavier français, et un raccourci écrit avec une lettre
  (`W-q`) doit rester sur cette lettre. Règle : comparer d'abord le keysym
  produit par la disposition active (avec et sans les modificateurs, en
  minuscule), puis, si aucun raccourci ne correspond, la touche physique (le
  keysym du niveau 1 de la disposition `us` pour ce code de touche).
- `allowWhenLocked="yes"` : actif aussi pendant le verrouillage (volume,
  luminosité).
- Les raccourcis sont prioritaires sur le client, sauf pendant le
  verrouillage et quand un client a demandé l'inhibition des raccourcis
  (protocole keyboard-shortcuts-inhibit), avec une échappatoire fixe : Super+Échap.

### 5.4 Liaisons de la souris

- `<context name="Title">` contient des `<mousebind button="Left"
  action="DoubleClick">`.
- Boutons : `Left`, `Middle`, `Right`, `Side`, `Extra`, avec modificateurs
  (`W-Left`) ; molette : `direction="Up|Down|Left|Right"` avec
  `action="Scroll"`.
- Événements : `Press`, `Release`, `Click` (appui et relâche sans
  déplacement), `DoubleClick`, `Drag`, `Scroll`.
- Contextes utilisés par Packarch : `Frame` (toute la vue), `Client`,
  `Title`, `TitleBar`, `Border` (et ses parties : `Top`, `Bottom`, `Left`,
  `Right`, `TLCorner`…), `Icon`, `WindowMenu`, `Iconify`, `Maximize`,
  `Close`, `Shade`, `AllDesktops`, `Root` (fond d'écran).
- Ordre : le contexte le plus précis est essayé d'abord (bouton, puis
  titre, puis barre de titre, puis cadre).

### 5.5 `menu.xml`

- `<menu id="root-menu" label="…">` contient des `<item label="…"
  icon="…">` avec actions, des `<separator/>` (avec `label` optionnel pour
  un titre), des sous-menus en ligne et des références `<menu id="…"/>` à un
  menu défini ailleurs.
- `execute="commande"` sur un `<menu>` : pipemenu. La commande est lancée par
  `sh -c` à chaque ouverture ; sa sortie est un `<openbox_pipe_menu>`
  contenant des items, séparateurs et menus. Délai limite de 5 secondes, puis
  menu « (vide) ». Pas de blocage de l'affichage pendant l'attente
  (exécution asynchrone, le menu s'ouvre à l'arrivée du résultat).
- Icônes : nom freedesktop (thème d'icônes de `theme/icon`) ou chemin.
- Menus fournis par le compositeur : `root-menu` (défaut minimal si absent),
  `client-menu` (menu de fenêtre : bureaux, toujours au-dessus, enrouler,
  maximiser, plein écran, décorations, fermer…), `client-list-combined-menu`
  (toutes les fenêtres, par bureau).
- Raccourcis d'accélérateur : `_` avant une lettre du libellé.

### 5.6 `environment`, `autostart`, `shutdown`

- `environment` : une variable par ligne, `NOM=valeur`, `#` pour les
  commentaires, `~` et `$VAR` développés en début de valeur. Relu à chaque
  rechargement ; s'applique aux programmes lancés ensuite.
  `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_OPTIONS`
  changent le clavier ; `XCURSOR_THEME` et `XCURSOR_SIZE`, le curseur.
- `autostart` : script `sh` lancé une fois, après la mise en place de
  l'affichage, avec `WAYLAND_DISPLAY` défini.
- `shutdown` : script lancé avant la fermeture de la session.
- Variables exportées aux clients : `WAYLAND_DISPLAY`, `DISPLAY` (XWayland),
  `XDG_CURRENT_DESKTOP=packarch:wlroots` (voir § 11), `PACKARCH_PID`.
  Ces variables sont aussi envoyées au gestionnaire de session de systemd
  et à D-Bus (`dbus-update-activation-environment --systemd`), pour les
  portails et les services.

### 5.7 Rechargement

- `packarch --reconfigure` (ou SIGHUP) relit `rc.xml`, `menu.xml`,
  `environment`, le thème et `themerc-override`, sans fermer les
  fenêtres.
- Une erreur de syntaxe XML garde l'ancienne configuration et affiche un
  message à l'écran (OSD de 5 secondes) et dans le journal.

---

## 6. Thèmes

### 6.1 Recherche

Le thème `theme/name` est cherché dans, dans l'ordre :
`$XDG_DATA_HOME/themes/NOM`, `~/.themes/NOM`, `/usr/share/themes/NOM`
(et les autres `$XDG_DATA_DIRS`). Fichier lu : `openbox-3/themerc`
(ou `packarch/themerc` s'il existe, prioritaire).
Puis `themerc-override` du dossier de configuration remplace clé par clé.

### 6.2 Format `themerc`

- Lignes `clé: valeur`, `#` et `!` pour les commentaires, espaces libres.
- Les clés peuvent contenir `*` comme joker (`window.*.border.color`), la
  clé la plus précise gagne.
- Couleurs : `#rgb`, `#rrggbb`, `#rrggbbaa`, et `#rrggbb 50` (opacité en %
  après la couleur).
- Textures `flat solid`, `raised gradient vertical`, etc. : seules les
  formes plates sont rendues fidèlement ; les dégradés `vertical`,
  `horizontal` et `diagonal` en deux couleurs (`.color` et `.colorTo`) sont
  rendus ; `raised`, `sunken`, `bevel` sont ignorés (rendus à plat).

### 6.3 Clés à prendre en charge

Toutes les clés présentes dans les thèmes `OB-Packarch-*` doivent avoir un
effet ou être ignorées sans erreur. Groupes :

- **Généralités** : `border.width`, `padding.width`, `padding.height`,
  `window.handle.width`, `window.client.padding.width|height`,
  `window.label.text.justify`.
- **Fenêtres** (pour `active` et `inactive`) : `border.color`,
  `title.bg(.color)`, `title.separator.color`, `label.text.color`,
  `label.text.font`, `client.color`, `handle.bg.color`, `grip.bg.color`.
- **Boutons** (pour `active`/`inactive` ; états `unpressed`, `hover`,
  `pressed`, `disabled`, `toggled.*`) : `button.STATE.bg(.color)`,
  `button.STATE.image.color`, et les variantes par bouton
  (`button.close.hover.image.color`, `button.max.toggled.unpressed.image.color`…).
- **Images de boutons** : `close`, `max`, `max_toggled`, `iconify`, `shade`,
  `shade_toggled`, `desk`, `desk_toggled`, avec suffixes `_hover`,
  `_pressed`. Formats : xbm (colorié avec `image.color`), png, svg, cherchés
  dans le dossier du thème. Images intégrées si absentes.
- **Menus** : `menu.border.width|color`, `menu.items.bg(.color)`,
  `menu.items.text.color`, `menu.items.active.bg.color`,
  `menu.items.active.text.color`, `menu.items.disabled.text.color`,
  `menu.items.active.disabled.text.color`, `menu.separator.color|width`,
  `menu.separator.padding.width|height`, `menu.title.bg.color`,
  `menu.title.text.color|justify`, `menu.overlap.x|y`, `menu.width.min|max`,
  `menu.items.padding.x|y`.
- **OSD** (Alt+Tab, bureaux) : `osd.bg.color`, `osd.border.color|width`,
  `osd.label.text.color`, `osd.hilight.bg.color`,
  `osd.active.label.*`, `osd.inactive.label.*`, `osd.button.*`.
- **Ombres des fenêtres** : `window.active.shadow.size`,
  `window.active.shadow.color`, `window.inactive.shadow.size`,
  `window.inactive.shadow.color` (couleur avec alpha `#rrggbbaa`).
- **Menus arrondis et ombrés** (clés propres à Packarch, mêmes noms que
  ceux qu'écrit déjà packarch-skin) : `menu.corner.radius` (px),
  `menu.shadow.size` (px), `menu.shadow.color` (`#rrggbbaa`).
  Ajouts : `osd.corner.radius`, `osd.shadow.size`, `osd.shadow.color` (par
  défaut : valeurs des menus).
- **Loupe** : `magnifier.border.width` (1 px par défaut, 0 : sans
  bordure), `magnifier.border.color` (par défaut : `menu.items.active.bg.color`,
  la couleur de surbrillance ; celle de la bordure des fenêtres se
  confondait avec leur fond dans 8 thèmes Packarch sur 10).

### 6.4 Polices

Les polices de `rc.xml` (`theme/font`) sont prioritaires sur celles du
thème. Recherche par fontconfig (nom de famille, graisse, style), taille en
points convertie selon l'échelle de l'écran. Rendu anticrénelé avec
sous-pixels désactivés (gris), pour rester correct sur fond transparent.

---

## 7. Actions

Chaque action est une structure Rust typée, construite à la lecture de la
configuration (les erreurs d'attributs sont signalées au chargement, pas à
l'exécution). Une action peut viser la vue sous le pointeur (souris) ou la
vue active (clavier, menu).

### 7.1 Utilisées par Packarch (priorité 1)

| Action | Attributs | Effet |
|---|---|---|
| `Execute` | `command` | Lance `sh -c commande`, détaché (nouveau groupe de processus, sortie vers le journal ou `/dev/null`), environnement de la session. |
| `ShowMenu` | `menu`, `atCursor` (yes/no), `x`, `y` | Ouvre un menu au pointeur ; `atCursor="no"` : sous le bouton ou au coin de la vue. |
| `Focus`, `Raise`, `Lower` | | Donne le focus, met au premier plan, à l'arrière-plan. |
| `Close` | | Demande la fermeture (xdg_toplevel.close / WM_DELETE_WINDOW). |
| `Kill` | | Termine le processus du client (SIGTERM puis SIGKILL après 2 s). |
| `Iconify` | | Réduit la vue (cachée, restaurée par la barre ou Alt+Tab). |
| `Maximize`, `ToggleMaximize`, `UnMaximize` | `direction` (both, horizontal, vertical) | Maximisation dans la zone utile (moins les panneaux et `margin`). |
| `ToggleFullscreen` | | Plein écran sur l'écran de la vue, sans cadre. |
| `ToggleDecorations` | | Montre ou cache le cadre de la vue. |
| `Shade` / `Unshade` / `ToggleShade` | | Enroule la vue : seule la barre de titre reste. |
| `ToggleAlwaysOnTop`, `ToggleAlwaysOnBottom` | | Couche d'empilement de la vue. |
| `ToggleOmnipresent` | | Vue visible sur tous les bureaux. |
| `Move`, `Resize` | | Déplacement ou redimensionnement interactif (souris), bord choisi selon la position du pointeur. |
| `MoveRelative` | `x`, `y` | Déplace de x, y pixels. |
| `ResizeRelative` | `left`, `right`, `top`, `bottom` | Agrandit ou réduit les bords. |
| `SnapToEdge` | `direction` (left, right, up, down, center), `combine` | Moitié d'écran (haut = maximiser), retour à la taille d'avant en sortant ; `combine=yes` : quarts (§ 15). |
| `SnapToRegion` | `region` | Place la vue dans une région de `rc.xml`. |
| `GoToDesktop` | `to` (1…n, left, right, last), `wrap` | Change de bureau. |
| `SendToDesktop` | `to`, `follow` (yes/no), `wrap` | Envoie la vue sur un bureau. |
| `NextWindow`, `PreviousWindow` | | Liste Alt+Tab (§ 8.3). |
| `Reconfigure` | | Recharge la configuration (§ 5.7). |
| `Exit` | | Quitte la session (après `shutdown`). |
| `AutoPlace` | `policy` | Replace la vue (règles de fenêtre). |

### 7.2 Utiles ensuite (priorité 2)

`MoveToEdge`, `GrowToEdge`, `ShrinkToEdge`, `MoveTo` (x, y),
`ResizeTo` (largeur, hauteur), `MoveToCursor`, `ToggleKeybinds`,
`FocusOutput`, `MoveToOutput`, `ToggleShowDesktop`, `If` / `ForEach`
(conditions sur le titre, l'identifiant, l'état), `None` (désactive un
raccourci par défaut), `ZoomIn`/`ZoomOut`/`ToggleMagnify` (loupe,
priorité 3, § 15).

---

## 8. Gestion des fenêtres

### 8.1 Empilement et focus

- Trois couches de vues : arrière (`AlwaysOnBottom`), normale, avant
  (`AlwaysOnTop`) ; sous elles les couches layer-shell `background` et
  `bottom`, au-dessus `top` puis `overlay`, comme l'exige le protocole.
- Focus au clic (`followMouse=no`) ; avec `followMouse=yes`, focus au survol,
  et mise au premier plan si `raiseOnFocus=yes`.
- Une nouvelle vue reçoit le focus et passe au premier plan, sauf si elle
  appartient à un autre bureau. Une demande d'activation (xdg-activation)
  d'un autre client marque la vue « urgente » sans voler le focus, sauf jeton
  valide d'une action récente de l'utilisateur.
- Fenêtres de dialogue (parent défini) : au-dessus de leur parent, centrées
  dessus, sur le même bureau.

### 8.2 Placement

- `automatic` : la position qui recouvre le moins les autres vues, en
  partant du coin haut gauche de la zone utile ; `center` ; `cursor` (sous le
  pointeur). Toujours entièrement dans la zone utile si la taille le permet.
- Une vue qui demande une position (XWayland) la garde si elle est visible.

### 8.3 Alt+Tab (liste des fenêtres)

- `NextWindow` ouvre la liste tant que le modificateur reste enfoncé ; la
  relâche donne le focus à la vue choisie. Échap annule.
- Style `classic` : un cadre centré, une ligne par vue, colonnes définies par
  `fields` (icône de l'application, nom de l'application d'après son fichier
  `.desktop`, titre), avec coins arrondis et ombre (clés `osd.*`).
- `preview="yes"` : la vue choisie passe au premier plan pendant le choix ;
  `outlines="yes"` : contour autour d'elle ; `unshade="yes"` : déroule
  temporairement une vue enroulée.
- Ordre : dernières vues utilisées d'abord, bureau courant seulement (vues
  omniprésentes comprises).

### 8.4 Bureaux

- Nombre et noms d'après `desktops/names`. Chaque vue appartient à un
  bureau, ou à tous (omniprésente).
- Changement de bureau : OSD avec le nom pendant `popupTime` ms.
- Exposé aux clients par le protocole ext-workspace (module
  `ext/workspaces` de waybar : affichage et clic pour changer).

### 8.5 Règles de fenêtres

- `<windowRule identifier="…" title="…" type="…">` : critères avec jokers
  `*` et `?`, sans casse. `identifier` est l'app_id Wayland ou la classe X11.
- Contenu : actions exécutées à la première apparition de la vue (`AutoPlace`,
  `ToggleAlwaysOnTop`, `SendToDesktop`, `Maximize`…), et propriétés :
  `serverDecoration`, `skipTaskbar`, `skipWindowSwitcher`, `fixedPosition`.
- Utilisé par Packarch : centrage de Waypaper et du sélecteur de thèmes.

### 8.6 Collage et résistance

- Pendant un déplacement, les bords de la vue « collent » aux bords de
  l'écran et des autres vues sur `screenEdgeStrength` et `windowEdgeStrength`
  pixels.
- Amener le pointeur à moins de `range` px d'un bord pendant un
  déplacement : aperçu translucide (coins arrondis), puis moitié d'écran ;
  bord haut et `topMaximize=yes` : maximisation. À moins de `cornerRange` px
  (50 par défaut, 0 pour ne jamais le faire) d'un coin le long de ce bord :
  quart d'écran.
- Déplacer une vue collée ou maximisée lui rend sa taille d'avant.

### 8.7 Redimensionnement

- Par les bords et les coins (zone de saisie d'au moins 8 px, même avec une
  bordure fine), ou Super+clic droit glissé (bord le plus proche).
- `popupShow` : OSD de la taille (en caractères pour les terminaux qui
  donnent un pas de redimensionnement, sinon en pixels).
- `drawContents=no` : seul un contour est dessiné pendant le geste.

---

## 9. Protocoles Wayland

### 9.1 Indispensables (version 0.1, prototype)

| Protocole | Pour |
|---|---|
| wl_compositor, wl_subcompositor, wl_shm, linux-dmabuf v4+ | Tous les clients |
| wl_seat, wl_output, xdg-output | Tous les clients, waybar |
| xdg-shell (toplevel, popup, positionner) | Applications |
| xdg-decoration, décoration KDE (cadre du compositeur quand le client le demande) | GTK, Qt, kitty |
| wlr-layer-shell | waybar, rofi, mako, swaybg, hyprlock (fond) |
| wl_data_device, primary-selection | Copier-coller |
| viewporter, fractional-scale | Mise à l'échelle 1,25 / 1,5 nette |
| presentation-time | Lecteurs vidéo, jeux |
| cursor-shape | Curseurs côté serveur |

### 9.2 Nécessaires pour l'usage quotidien (version 0.3)

| Protocole | Pour |
|---|---|
| ext-session-lock | hyprlock (verrouillage) |
| ext-idle-notify, idle-inhibit | swayidle, vidéos en plein écran |
| wlr-foreign-toplevel-management, ext-foreign-toplevel-list | Module `wlr/taskbar` de waybar |
| ext-workspace | Module `ext/workspaces` de waybar |
| wlr-screencopy, ext-image-capture-source, ext-image-copy-capture | grim, wf-recorder, hyprpicker |
| wlr-output-management | wlr-randr, kanshi |
| wlr-output-power-management | wlopm (extinction des écrans) |
| wlr-data-control, ext-data-control | wl-copy, gestionnaires de presse-papiers |
| xdg-activation | Focus entre applications (liens, notifications) |
| pointer-constraints, relative-pointer | Jeux (packarch-spider, etc.) |
| text-input v3, input-method v2 | Méthodes de saisie |
| virtual-keyboard, virtual-pointer | wtype, outils d'automatisation, tests |
| keyboard-shortcuts-inhibit | Machines virtuelles, bureau à distance |
| single-pixel-buffer, alpha-modifier, content-type | Clients récents |
| security-context | Flatpak |
| xdg-dialog, xdg-toplevel-icon | Dialogues modaux, icônes de fenêtre |
| tearing-control | Jeux (si `adaptiveSync`) |

### 9.3 Portails

Pas de portail intégré. Packarch installe `xdg-desktop-portal-wlr` (capture
d'écran, partage d'écran dans Firefox) et `xdg-desktop-portal-gtk` (choix de
fichiers) ; le compositeur fournit les protocoles qu'ils demandent
(screencopy, output) et un fichier `packarch-portals.conf`.

### 9.4 XWayland

- Lancé à la première connexion d'un client X11 (`xwaylandPersistence=no`)
  ou au démarrage (`yes`).
- Gestionnaire de fenêtres X11 intégré (Smithay `X11Wm`) : cadres,
  focus, empilement, fenêtres « override-redirect » (menus X11) sans cadre,
  `_NET_WM_STATE` (plein écran, maximisé), `WM_TRANSIENT_FOR`, icônes
  `_NET_WM_ICON`, classes pour les règles.
- Mise à l'échelle : par défaut, les clients X11 sont rendus à l'échelle 1
  puis agrandis (flou acceptable). Avec `<core><xwaylandSharp>yes`, ils
  sont rendus à la plus grande échelle des écrans et restent nets (§ 15).

---

## 10. Rendu

### 10.1 Cadre des fenêtres

- Barre de titre : hauteur = hauteur de la police + 2 × `padding.height`,
  disposition des boutons d'après `titlebar/layout`
  (`gauche:droite`, éléments `icon`, `menu`, `iconify`, `max`, `close`,
  `shade`, `desk`, séparés par des virgules, ex. `:iconify,max,close`).
- Titre centré, à gauche ou à droite (`window.label.text.justify`), tronqué
  avec « … ».
- Bordure de `border.width` px de `border.color` ; `keepBorder=yes` garde
  la bordure quand le cadre est caché par `ToggleDecorations`.
- Survol et appui des boutons : couleurs et images d'après le thème (§ 6.3).

### 10.2 Coins arrondis

- Rayon `theme/cornerRadius` pour les fenêtres, `menu.corner.radius` pour
  les menus, `osd.corner.radius` pour les OSD.
- Fenêtres : arrondi des deux coins du haut de la barre de titre et des deux
  coins du bas. Le contenu du client est découpé par un masque
  anticrénelé dans le shader de rendu, pour que les coins du bas soient
  arrondis même si le client dessine jusqu'au bord.
- Pas d'arrondi pour une vue maximisée, collée à un bord ou en plein écran.
- Anticrénelage sur 1 px physique, correct à toutes les échelles (1, 1,25,
  1,5, 2) sans trou entre bordure et contenu.

### 10.3 Ombres

- Ombre portée sous les vues, les menus, les sous-menus et les OSD, de taille
  `*.shadow.size` et de couleur `*.shadow.color` (alpha compris), sans
  décalage (centrée), qui suit les coins arrondis.
- Calcul : flou gaussien approché par la fonction d'erreur d'un rectangle
  arrondi, dans un shader (aucune texture à recalculer quand la taille de la
  vue change). Repli pixman : texture neuf-parties mise en cache par
  (taille, rayon, couleur).
- `dropShadows=no` : aucune ombre de fenêtre. `dropShadowsOnTiled=no` : pas
  d'ombre sur les vues maximisées ou collées.
- L'ombre n'intercepte pas les clics (les clics y passent à la vue
  dessous) et n'agrandit pas la zone de saisie des bords au-delà de 8 px.
- L'ombre ne déborde pas d'un écran sur l'autre de façon visible (rognée par
  écran).

### 10.4 Menus

- Fond, bordure, séparateurs, titres de section, élément survolé d'après le
  thème ; icône à gauche (16 px logiques), flèche des sous-menus à droite.
- Largeur : celle du plus long libellé, bornée par `menu.width.min|max`,
  le texte au-delà est tronqué avec « … ».
- Sous-menu : ouvert au survol après 100 ms, à droite du menu parent
  (décalé de `menu.overlap.x|y`), ou à gauche s'il ne tient pas ; vers le
  haut s'il dépasse le bas de l'écran.
- Clavier : flèches, Entrée, Échap, Origine/Fin, lettres d'accélérateur.
- Le menu reste entièrement sur l'écran du pointeur. Un clic hors du menu le
  ferme sans transmettre le clic.

### 10.5 Animations

Sobres, courtes, jamais bloquantes : une action reste immédiate (le focus,
la position finale et les événements d'entrée changent tout de suite), seule
l'image est animée. Une nouvelle action pendant une animation la remplace en
partant de l'état affiché.

| Animation | Effet | Durée par défaut |
|---|---|---|
| Ouverture d'une vue | Fondu de 0 à 1 et agrandissement de 94 % à 100 %, depuis le centre | 160 ms |
| Fermeture d'une vue | Inverse, sur la dernière image du client gardée en mémoire | 140 ms |
| Réduction et restauration | Fondu et rétrécissement vers le bas de l'écran (ou vers l'entrée de la barre des tâches si connue) | 180 ms |
| Maximisation, collage, région | Passage du rectangle de départ au rectangle final, contenu étiré puis remplacé par la nouvelle image du client | 180 ms |
| Ouverture et fermeture des menus et sous-menus | Fondu et glissement de 4 px | 100 ms |
| Alt+Tab et OSD | Fondu | 100 ms |
| Changement de bureau | Glissement horizontal des vues (gauche/droite selon le numéro) | 220 ms |
| Aperçu de collage | Fondu et croissance du rectangle translucide | 120 ms |
| Enroulement | Hauteur animée jusqu'à la barre de titre | 140 ms |

- Courbes : `ease-out-cubic` par défaut ; `linear`, `ease-in-out-cubic`,
  et ressort amorti (`spring`, avec raideur et amortissement) disponibles.
- Configuration dans `rc.xml` :

  ```xml
  <animations enabled="yes" speed="1.0">
    <window-open duration="160" curve="ease-out-cubic" />
    <workspace-switch duration="220" curve="spring" />
    <menu enabled="no" />
  </animations>
  ```

  `speed` multiplie toutes les durées (0,5 = deux fois plus rapide) ;
  `enabled="no"` ou `--no-animations` désactive tout. Éléments, dans l'ordre
  du tableau : `window-open`, `window-close`, `minimize`, `maximize`,
  `menu`, `osd`, `workspace-switch`, `snap-preview`, `shade` ; attributs
  `enabled`, `duration`, `curve`, et pour `spring` : `stiffness` (600 par
  défaut) et `damping` (40).
- Désactivées automatiquement avec le backend pixman (pas de GPU) et en
  plein écran direct ; les ombres et coins suivent l'animation.
- Horloge : celle des rafraîchissements d'écran (presentation feedback),
  pas l'horloge murale ; un écran à 144 Hz anime à 144 images par seconde.
- Tests : horloge simulée, pour des captures reproductibles à un instant
  donné de l'animation.

### 10.6 Performances

- Rendu par dommage (seules les zones modifiées sont redessinées).
- Plein écran direct (« direct scanout ») quand une seule vue opaque couvre
  l'écran.
- Pas plus d'un rendu par rafraîchissement d'écran ; aucune image perdue
  pendant un déplacement de fenêtre ou une animation à 60 Hz sur un GPU
  intégré.

---

## 11. Intégration à Packarch (plus tard)

Aucun paquet de Packarch n'est modifié tant que le compositeur n'a pas
atteint la version 0.5 (§ 14). Prévu ensuite :

- Paquet `packarch-rwdesktop` (programme `/usr/bin/packarch`, fichier de
  session `/usr/share/wayland-sessions/packarch.desktop`, portails,
  configuration par défaut dans `/etc/xdg/packarch/`).
- `XDG_CURRENT_DESKTOP=packarch:wlroots`. La partie `wlroots`
  permet aux outils (portail wlr, waybar…) de reconnaître un compositeur qui
  fournit les protocoles wlr.
- packarch-skin : nouvelle cible qui écrit dans
  `~/.config/packarch/` (mêmes fichiers qu'aujourd'hui) et lance
  `packarch --reconfigure`.
- packarch-labwc-settings : soit adapté, soit doublé par
  `packarch-rwdesktop-settings` (mêmes scripts, menus et barres).
- Calamares et packarch-depannage : choix du bureau.

---

## 12. Ligne de commande et interfaces

```
packarch [OPTIONS]
  -C, --config-dir DOSSIER   dossier de configuration
  -s, --startup COMMANDE     commande lancée après autostart
  -r, --reconfigure          demande à l'instance en cours de recharger
  -e, --exit                 demande à l'instance en cours de quitter
  -v, --version              version (avec le commit git)
  -d, --debug                journal détaillé
      --backend udev|winit|headless   (par défaut : winit dans une session graphique, udev sinon)
      --no-animations            désactive les animations (§ 10.5)
```

- L'instance en cours est trouvée par `PACKARCH_PID`, puis par un fichier
  `$XDG_RUNTIME_DIR/packarch-$WAYLAND_DISPLAY.pid`.
- Signaux : SIGHUP recharge, SIGTERM quitte proprement.
- Journal : sortie d'erreur, niveau `info` ; `RUST_LOG` respecté.

---

## 13. Qualité et tests

- **Tests unitaires** : lecture de `rc.xml`, `menu.xml`, `themerc` (tous les
  thèmes OB-Packarch-* doivent se lire sans erreur), couleurs, raccourcis
  AZERTY, placement, collage, régions.
- **Tests d'intégration sans écran** : backend headless + pixman, clients de
  test (wayland-client en Rust), touches et clics simulés par
  virtual-keyboard / virtual-pointer, captures par screencopy comparées à des
  images de référence (tolérance de quelques niveaux). Cas : menu racine
  ouvert, sous-menu, Alt+Tab, fenêtre maximisée, coins et ombres à l'échelle
  1 et 1,5.
- **Essais réels** : sur la machine de développement en session imbriquée
  (winit), puis sur packarch-test en session complète.
- **CI** : `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test`, `cargo deny check`.
- **Robustesse** : aucune panique sur une entrée client invalide (erreur de
  protocole envoyée au client) ; `panic = "abort"` désactivé au profit d'une
  journalisation de la panique avant la sortie, pour retrouver la cause.

---

## 14. Étapes

| Version | Contenu | Critère de fin |
|---|---|---|
| **0.1 prototype** | Backends winit et headless, xdg-shell, layer-shell, cadre simple, coins arrondis et ombres des fenêtres, thème OB lu, waybar et swaybg affichés | kitty et Thunar déplaçables dans une session imbriquée, ombres et coins corrects sur capture |
| **0.2 configuration** | `rc.xml` complet pour les sections **P**, raccourcis (AZERTY), souris, `environment`, `autostart`, rechargement | Le `rc.xml` de Packarch se charge sans avertissement, tous ses raccourcis répondent |
| **0.3 menus** | `menu.xml`, pipemenus, menus de fenêtre et liste des fenêtres, arrondis et ombrés ; moteur d'animation, animations des menus et des fenêtres | Le menu de Packarch (Super+Espace) fonctionne en entier, pipemenus compris |
| **0.4 session** | Backend udev, plusieurs écrans, verrouillage, inactivité, capture, barre des tâches, bureaux (avec glissement), Alt+Tab | Une journée de travail sur packarch-test sans retour à labwc |
| **0.5 XWayland** | X11Wm, règles de fenêtres, collage et régions, portails | Steam, LibreOffice et un jeu X11 utilisables ; début de l'intégration (§ 11) |
| **1.0** | Priorité 2 des actions, finitions, documentation `man` | Bureau par défaut de Packarch |
| **1.1** | `xwaylandSharp`, quarts d'écran (`combine`), loupe, images de référence (§ 13) | Rendu protégé par les scénarios de référence |

---

## 15. Décisions et questions ouvertes

Décidé le 17 novembre 2026 :

1. Programme `packarch` (paquet `packarch-rwdesktop`).
2. Dossier de configuration propre : `~/.config/packarch/`.
3. Animations prévues (§ 10.5), dès la version 0.3.

4. Racine de `rc.xml` : `<packarch_config>` dans les fichiers de Packarch ;
   `<openbox_config>` et `<labwc_config>` acceptées en lecture (§ 5.2).
5. Sous-menus : `menu.overlap.x` positif les fait chevaucher leur parent,
   négatif laisse un espace ; `menu.overlap.y` les décale vers le bas.
   Valeurs par défaut des clés de menu absentes du thème : bordure
   `border.width`, marges 7 × 4 px, largeur 20 à 200 px, rayon 6 px.
6. Session réelle : seul le GPU principal dessine ; les écrans sont posés
   côte à côte de gauche à droite à l'échelle 1, puis réglés par
   wlr-randr ou kanshi (wlr-output-management). Le choix du backend par
   défaut suit l'environnement : `winit` si `WAYLAND_DISPLAY` ou `DISPLAY`
   est défini, `udev` sinon.
7. Alt+Tab : Échap revient à la vue de départ. Colonne
   `desktop_entry_name` : nom du fichier `.desktop` correspondant à
   l'identifiant de la vue (nom traduit selon `LANG`).
8. XWayland est lancé au démarrage, quel que soit `xwaylandPersistence`
   (le démarrage à la première connexion X11 viendra plus tard) ;
   `autostart` attend qu'il soit prêt (3 s au plus) pour que `DISPLAY` soit
   défini. Les fenêtres X11 sans indication Motif reçoivent le cadre du
   compositeur ; celles qui demandent « sans décoration » (Steam…) non.
9. Actions de priorité 2 : `ShrinkToEdge` rentre le bord donné jusqu'au
   bord d'une autre fenêtre à l'intérieur, sinon de moitié ; `MoveTo` est
   relatif à la zone utile ; `ResizeTo` donne la taille du client.
   Requêtes `If`/`ForEach` : `identifier`, `title` (jokers), `maximized`,
   `iconified`, `focused`, `omnipresent`, `shaded`, `fullscreen`,
   `desktop` ; plusieurs `<query>` : l'une suffit.
10. `popupShow` : la taille est affichée au centre de la fenêtre ; en
    caractères seulement pour les clients X11 qui donnent un pas de
    redimensionnement (les clients Wayland n'en donnent pas).
11. Le protocole xdg-toplevel-icon n'est pas proposé : avec Smithay 0.7,
    il empêche certaines applications Qt de s'afficher.

12. XWayland à la première connexion X11 : pas possible avec Smithay 0.7.
    `XWayland::spawn` lance le serveur tout de suite, et le gestionnaire
    X11 de Smithay exige les données de client `XWaylandClientData`, que
    seul `spawn` sait créer. À reprendre quand Smithay le permettra.
13. `xwaylandSharp` (propre à Packarch, `no` par défaut) : XWayland reçoit
    une échelle client égale à la plus grande échelle des écrans ; les
    clients X voient les écrans en pixels réels. Le DPI leur est donné par
    XSettings (`Xft/DPI`, `Gdk/WindowScalingFactor` entier,
    `Gdk/UnscaledDPI`, `Xcursor/size`) et par `Xft.dpi` (xrdb, s'il est
    installé). Appliqué à chaud (écrans, rechargement). Les applications X
    qui ne lisent pas le DPI paraissent plus petites : d'où `no` par
    défaut.
14. Quarts d'écran (`SnapToEdge combine="yes"`, comme labwc) : la nouvelle
    direction remplace celle du même axe et garde l'autre. Gauche puis
    haut donne le quart en haut à gauche, puis bas le quart en bas à
    gauche ; une moitié puis la direction opposée donne l'autre moitié.
    Une vue maximisée (haut) puis gauche va dans le quart en haut à gauche.
    Une vue libre, maximisée par l'action `Maximize` ou au centre prend le
    bord demandé.
15. Loupe : `ZoomIn` l'ouvre à `initScale` (2 par défaut) puis grossit de
    `increment` (0,5), jusqu'à 16 ; `ZoomOut` réduit d'autant et la ferme
    en arrivant à 1 ; `ToggleMagnify` l'ouvre ou la ferme. Rectangle de
    `width` × `height` px (400 × 400) centré sur le pointeur, sur l'écran
    où il se trouve, avec une bordure du thème ; 0 pour l'une des tailles :
    tout l'écran. Le point sous le pointeur reste sous le pointeur, si bien
    que les clics vont à ce qu'on voit agrandi. Image de l'écran rendue
    dans une texture puis agrandie (lissée, ou pixels nets avec
    `useFilter=no`) ; le curseur n'est pas agrandi.

16. Images de référence (§ 13) : client de test `client-essai` (exemple
    Cargo, fenêtre xdg-shell d'une couleur unie qui demande le cadre du
    compositeur) ; configuration figée dans `tests/essai` (thème Essai,
    copie d'OB-Packarch-Packarch, polices DejaVu Sans, sans fond ni barre,
    sans icône, clavier `us`) ; étape `compare` des scénarios marqués
    `#reference`, références dans `tests/references`, écart nul exigé par
    défaut (le rendu headless est déterministe), `tests/capture.sh --update`
    pour les refaire. Les captures dépendent des polices installées : les
    références se refont sur une autre machine.

17. Cadre des fenêtres (comme labwc) : le mode demandé par le client
    (xdg-decoration, décoration KDE de GTK3) est respecté ; une application
    à GtkHeaderBar (Packarch Hello, Firefox) dessine seule sa barre de
    titre. `core/decoration` ne vaut que pour les clients sans préférence,
    et donne le mode par défaut annoncé à GTK3 ; la règle de fenêtre
    `serverDecoration` impose ou retire le cadre.

18. Curseur des machines virtuelles : depuis Linux 6.8, virtio-gpu, qxl et
    vmwgfx cachent leur plan curseur aux compositeurs atomiques qui ne
    s'engagent pas à donner le point chaud du curseur
    (`DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`, `HOTSPOT_X`/`HOTSPOT_Y`).
    Smithay 0.7 ne le fait pas : le curseur était dessiné dans l'image et
    SPICE affichait le sien par-dessus (deux curseurs). Copie modifiée de
    Smithay 0.7.0 dans `vendor/smithay` (`[patch.crates-io]`, détail dans
    `vendor/README.md`), à retirer quand Smithay le gérera.
19. Ouverture de la session sous LightDM : la session libseat n'est activée
    qu'après le lancement du compositeur, quand le greeter rend l'écran.
    Ouvrir le GPU avant échouait (« Unable to become drm master »,
    Permission denied) et renvoyait à l'écran de connexion. packarch attend
    l'activation, 10 s au plus, avant d'ouvrir le GPU, comme wlroots.

Aucune question ouverte pour l'instant.
