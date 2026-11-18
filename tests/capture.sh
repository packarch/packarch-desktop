#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Lance des scénarios d'essai sans écran et écrit leurs captures (§ 13).
#   tests/capture.sh [SCÉNARIO…]       (par défaut: tous ceux de tests/scenarios)
# Dossier des captures: target/captures/NOM. Configuration: le rc.xml de
# ~/.config/packarch, sinon celui de labwc, résistance des bords désactivée;
# autostart d'essai: fond uni (swaybg) et barre du thème (waybar, écran
# HEADLESS-1). Code de sortie non nul si une vérification échoue.
# Une ligne « #! OPTIONS » du scénario ajoute des options au programme
# (par exemple « #! --size 1920x1200 --scale 1.5 »).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
(($#)) || set -- "$root"/tests/scenarios/*.txt
src="$HOME/.config/packarch"
[[ -f $src/rc.xml ]] || src="$HOME/.config/labwc"
cargo build --quiet --manifest-path "$root/Cargo.toml"
status=0
for scenario in "$@"; do
	scenario="$(realpath "$scenario")"
	name="$(basename "$scenario" .txt)"
	out="$root/target/captures/$name"
	rm -rf "$out"
	mkdir -p "$out/cfg" "$out/waybar"
	sed -e 's|<screenEdgeStrength>[0-9]*<|<screenEdgeStrength>0<|' \
		-e 's|<windowEdgeStrength>[0-9]*<|<windowEdgeStrength>0<|' "$src/rc.xml" >"$out/cfg/rc.xml"
	for f in themerc-override environment menu.xml; do
		[[ -f $src/$f ]] && cp "$src/$f" "$out/cfg/"
	done
	{
		echo "swaybg -c '#c8ccd0' &"
		if [[ -f $src/waybar/config ]]; then
			sed -e 's/"@output@"/"HEADLESS-1"/' -e '/@cpu-temp@/d' -e 's/@marge15@/0/g' \
				"$src/waybar/config" >"$out/waybar/config"
			cp "$src/waybar/"*.css "$out/waybar/"
			echo "waybar -c '$out/waybar/config' -s '$out/waybar/style.css' >waybar.log 2>&1 &"
		fi
	} >"$out/cfg/autostart"
	echo "== $name"
	read -ra opts <<<"$(sed -n 's/^#! //p' "$scenario")"
	if (cd "$out" && "$root/target/debug/packarch" --backend headless -C "$out/cfg" "${opts[@]}" --script "$scenario" >"$out/journal.txt" 2>&1); then
		echo "   réussi, captures dans $out"
	else
		status=1
		grep -E "ÉCHEC|ERROR" "$out/journal.txt" | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^/   /'
	fi
done
exit $status
