# Vidéo saccadée dans tous les lecteurs (corrigé en 1.3.3)

## Symptôme

De la version 1.3.0 à la 1.3.2, la vidéo saccadait dans tous les lecteurs :
mpv (Wayland natif), SMPlayer, VLC et Nounours Player (par XWayland).
Machine concernée : NVIDIA RTX 4080 SUPER (pilote propriétaire), deux
écrans 1920×1080 à 60 Hz.

## Mesure

mpv lit une vidéo de test à 23,976 i/s pendant 9 s, avec `WAYLAND_DEBUG=1`.
Dans la trace, on compte les événements `wp_presentation_feedback.presented`
et l'écart entre leurs heures de vblank :

| | Images présentées | Écarts entre images |
|---|---|---|
| Attendu | ~215 | 33 / 50 ms (cadence 2:3 sur 60 Hz) |
| 1.3.2 | 126 | 67, 83, 100 ms |

mpv validait pourtant une image toutes les 42 ms. Mais son rappel de trame
(`wl_callback.done`) et ses retours de présentation n'arrivaient qu'au
moment où mpv envoyait sa requête suivante.

## Causes

### 1. Un commit débloqué ne demandait pas de rendu

Depuis la 1.3.0 (synchronisation explicite, décision 22 du cahier des
charges), le commit d'un tampon dmabuf est retenu par un *blocker*. Il est
libéré quand le point d'acquisition du client est signalé, ou à défaut la
barrière implicite du dmabuf (`add_commit_blockers`, `src/handlers.rs`).

La source d'événements qui lève le blocker appelait seulement
`blocker_cleared`. Le commit était alors appliqué, mais **aucun rendu
n'était demandé**. Seul le traitement des requêtes Wayland appelle
`request_redraw` (`src/state.rs`). La nouvelle image restait donc invisible
jusqu'au message suivant du client, environ une image plus tard. Une image
sur deux sautait ou arrivait avec une période de retard.

Le pilote NVIDIA ne fait que de la synchronisation explicite, donc tout
tampon GPU passe par ce chemin. Ça touche toutes les applications qui
dessinent avec le GPU, XWayland compris, d'où la saccade dans tous les
lecteurs.

### 2. Les événements n'étaient envoyés qu'après un rendu

`flush_clients` n'était appelé qu'à la fin de `after_render`
(`src/backend/mod.rs`). Les retours de présentation sont écrits au vblank
(`udev_vblank`, `src/backend/udev.rs`). Ils restaient dans le tampon de
sortie jusqu'au rendu suivant, ou jusqu'à ce qu'une requête du client
réveille la boucle. Les heures de vblank arrivaient donc en retard. Les
libérations de tampons et les rappels envoyés hors rendu attendaient aussi.

## Corrections (1.3.3)

- `src/handlers.rs` : la fermeture `release` appelle
  `state.request_redraw()` après `blocker_cleared`.
- `src/main.rs` : `event_loop.run` vide les tampons des clients
  (`state.dh.flush_clients()`) à chaque tour de boucle, comme le fait anvil,
  le compositeur d'exemple de Smithay.

## Vérifier

1. Générer une vidéo de test :
   `ffmpeg -f lavfi -i testsrc2=size=1920x1080:rate=24000/1001 -t 12 -c:v libx264 -preset ultrafast t24.mp4`
2. La lire : `WAYLAND_DEBUG=1 mpv --no-config t24.mp4 2> wl.log`
3. Compter les lignes `.presented(` et calculer les écarts entre leurs
   heures (3ᵉ argument : `tv_nsec`, 2ᵉ : `tv_sec_lo`).

Résultat attendu : environ 24 images par seconde, avec des écarts
alternés de 33 et 50 ms.

## Remarques

- Le numéro de séquence des retours de présentation vaut 0 avec le pilote
  NVIDIA (`DrmEventMetadata::sequence`). Sans conséquence observée.
- Piste encore ouverte (voir `TODO.md`) : le contenu des fenêtres arrondies
  change d'identifiant à chaque image, donc toute la fenêtre est sans doute
  redessinée à chaque rendu. C'est un coût GPU, pas une cause de saccade.
