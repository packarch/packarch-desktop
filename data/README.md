# Fichiers d'intégration

| Fichier | Installé dans | Rôle |
|---|---|---|
| `packarch.desktop` | `/usr/share/wayland-sessions/` | Session « Packarch » du gestionnaire de connexion |
| `../docs/packarch.1` | `/usr/share/man/man1/` | Page de manuel |
| `packarch-portals.conf` | `/usr/share/xdg-desktop-portal/` | Portails : capture et partage d'écran par xdg-desktop-portal-wlr, le reste par xdg-desktop-portal-gtk |

Le programme `packarch` exporte `XDG_CURRENT_DESKTOP=packarch:wlroots`,
`WAYLAND_DISPLAY` et `DISPLAY` vers systemd et D-Bus au démarrage d'une
session réelle (`dbus-update-activation-environment --systemd`).
