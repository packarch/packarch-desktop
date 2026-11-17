#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Lance un scénario d'essai sans écran et écrit ses captures (§ 13).
#   tests/capture.sh [SCÉNARIO] [DOSSIER] [options de packarch…]
# Configuration: ~/.config/packarch, sinon une copie de celle de labwc
# (rc.xml, themerc-override); barre: waybar du thème courant, écran
# HEADLESS-1.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
scenario="$(realpath "${1:-$root/tests/scenarios/prototype.txt}")"
out="$(realpath -m "${2:-$root/target/captures}")"
shift $(( $# > 2 ? 2 : $# ))
mkdir -p "$out/cfg" "$out/waybar"
src="$HOME/.config/packarch"
[[ -f $src/rc.xml ]] || src="$HOME/.config/labwc"
cp "$src/rc.xml" "$out/cfg/"
[[ -f $src/themerc-override ]] && cp "$src/themerc-override" "$out/cfg/"
if [[ -f $src/waybar/config ]]; then
	sed -e 's/"@output@"/"HEADLESS-1"/' -e '/@cpu-temp@/d' -e 's/@marge15@/0/g' \
		"$src/waybar/config" >"$out/waybar/config"
	cp "$src/waybar/"*.css "$out/waybar/"
fi
cargo build --quiet --manifest-path "$root/Cargo.toml"
cd "$out"
"$root/target/debug/packarch" --backend headless -C "$out/cfg" --script "$scenario" "$@"
echo "Captures: $out"
