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
