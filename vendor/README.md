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

À retirer quand Smithay gérera le point chaud du plan curseur.
