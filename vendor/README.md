# Smithay 0.7.0 modifié

Copie de [Smithay](https://github.com/Smithay/smithay) 0.7.0 (licence MIT,
`smithay/LICENSE.txt`), utilisée à la place de celle de crates.io par
`[patch.crates-io]` dans `Cargo.toml`. Modifications, marquées `packarch:` :

- `src/backend/drm/device/mod.rs` : capacité
  `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT` après le mode atomique. Sans elle,
  depuis Linux 6.8, les pilotes virtuels (virtio-gpu, qxl, vmwgfx) cachent
  leur plan curseur : le compositeur dessinait le curseur dans l'image et
  la fenêtre SPICE affichait le sien par-dessus (deux curseurs).
- `src/backend/drm/surface/{mod,atomic}.rs` : champ `hotspot` de
  `PlaneConfig`, envoyé dans `HOTSPOT_X` / `HOTSPOT_Y`.
- `src/backend/drm/compositor/mod.rs` : `DrmCompositor::set_cursor_hotspot`,
  point chaud de l'élément curseur, donné au plan curseur.
- `src/backend/renderer/gles/element.rs` : `TextureShaderElement::with_id`
  et `PixelShaderElement::with_id`. `new` donne un nouvel identifiant à
  chaque appel : le suivi des dégâts prenait le contenu des fenêtres
  arrondies, leur cadre et leur ombre pour de nouveaux éléments à chaque
  image et les redessinait en entier.
- `src/xwayland/xwm/dnd.rs` (nouveau) et `src/xwayland/xwm/mod.rs` :
  glisser-déposer XDND entre Wayland et X11 (décision 32 du cahier des
  charges). `XdndSelection` est une troisième sélection (`SelKind::Dnd`)
  qui réutilise les transferts des presse-papiers (un transfert terminé
  quitte la liste : laissé, il était pris pour périmé à la demande
  suivante du même client et recevait une réponse d'échec, que Java lisait
  comme celle de sa nouvelle demande) ; méthodes
  `X11Wm::dnd_*` et `X11Wm::x11_drag_*`, fenêtre mandataire, nouvelles
  méthodes de `XwmHandler` (par défaut sans effet).
- `src/wayland/selection/data_device/dnd_grab.rs` et `mod.rs` :
  `ClientDndGrabHandler::foreign_dnd_target`, `foreign_dnd_motion`,
  `foreign_dnd_drop` : la saisie DnD laisse au compositeur les surfaces
  qu'il relaie lui-même (XWayland).
- `src/wayland/selection/data_device/foreign_dnd.rs` (nouveau) :
  `ForeignDnd`, offre de glisser-déposer d'une source étrangère (client
  X11) à un client Wayland, sans saisie du pointeur ; réponses par
  `ForeignDndHandler`.

À retirer quand Smithay gérera le point chaud du plan curseur et le
glisser-déposer XWayland.
