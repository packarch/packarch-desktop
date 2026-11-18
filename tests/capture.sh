#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Lance des scénarios d'essai sans écran et écrit leurs captures (§ 13).
#   tests/capture.sh [--update] [SCÉNARIO…]   (par défaut: tous ceux de tests/scenarios)
# Dossier des captures: target/captures/NOM. Configuration: le rc.xml de
# ~/.config/packarch, sinon celui de labwc, résistance des bords désactivée;
# autostart d'essai: fond uni (swaybg) et barre du thème (waybar, écran
# HEADLESS-1). Code de sortie non nul si une vérification échoue.
# Une ligne « #! OPTIONS » du scénario ajoute des options au programme
# (par exemple « #! --size 1920x1200 --scale 1.5 »).
#
# Une ligne « #reference » rend le scénario indépendant de la machine:
# configuration figée de tests/essai (thème Essai, sans fond ni barre),
# fenêtres du client de test (examples/client-essai.rs, dans le PATH), et
# les étapes « compare X ref/X » comparent les captures aux images de
# tests/references/NOM. --update remplace ces images par les captures.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
update=0
if [[ ${1:-} == --update ]]; then
	update=1
	shift
fi
(($#)) || set -- "$root"/tests/scenarios/*.txt
src="$HOME/.config/packarch"
[[ -f $src/rc.xml ]] || src="$HOME/.config/labwc"
cargo build --quiet --manifest-path "$root/Cargo.toml"
cargo build --quiet --manifest-path "$root/Cargo.toml" --example client-essai
status=0
for scenario in "$@"; do
	scenario="$(realpath "$scenario")"
	name="$(basename "$scenario" .txt)"
	out="$root/target/captures/$name"
	rm -rf "$out"
	mkdir -p "$out/cfg" "$out/waybar"
	if grep -q '^#reference' "$scenario"; then
		refs="$root/tests/references/$name"
		mkdir -p "$refs"
		ln -s "$refs" "$out/ref"
		cp "$root"/tests/essai/{rc.xml,menu.xml} "$out/cfg/"
		: >"$out/cfg/autostart"
		echo "== $name (références)"
		read -ra opts <<<"$(sed -n 's/^#! //p' "$scenario")"
		if (cd "$out" && XDG_DATA_HOME="$root/tests/essai/data" XKB_DEFAULT_LAYOUT=us \
			PATH="$root/target/debug/examples:$PATH" \
			"$root/target/debug/packarch" --backend headless -C "$out/cfg" "${opts[@]}" \
			--script "$scenario" >"$out/journal.txt" 2>&1); then
			echo "   réussi, captures dans $out"
		elif ((update)); then
			echo "   écarts ignorés (--update)"
		else
			status=1
			grep -E "ÉCHEC|ERROR" "$out/journal.txt" | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^/   /'
		fi
		if ((update)); then
			sed -n 's|^compare \([^ ]*\) ref/\([^ ]*\).*|\1 \2|p' "$scenario" |
				while read -r got ref; do
					cp "$out/$got" "$refs/$ref"
				done
			echo "   références mises à jour dans $refs"
		fi
		continue
	fi
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
