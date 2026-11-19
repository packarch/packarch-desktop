#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Vérifications avant publication (cahier des charges § 13) :
#   tests/ci.sh [--sans-scenarios | --logiciel]
# Formatage, clippy sans avertissement, tests unitaires, licences et
# avis de sécurité (cargo deny, s'il est installé), puis scénarios de
# référence (tests/capture.sh, rendu headless sur le GPU ; --logiciel :
# rendu par le processeur, Mesa llvmpipe, sans GPU).
# S'arrête à la première étape en échec.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
scenarios=1
capture=()
case ${1:-} in
--sans-scenarios) scenarios=0 ;;
--logiciel) capture=(--logiciel) ;;
esac

etape() { printf '\n== %s\n' "$*"; }

etape "cargo fmt --check"
cargo fmt --check

etape "cargo clippy -D warnings"
cargo clippy --quiet --all-targets -- -D warnings

etape "cargo test"
cargo test --quiet

etape "cargo deny check"
if command -v cargo-deny >/dev/null; then
	cargo deny check
else
	echo "   cargo-deny absent, étape sautée (pacman -S cargo-deny)"
fi

if ((scenarios)); then
	etape "scénarios de référence"
	mapfile -t refs < <(grep -l '^#reference' tests/scenarios/*.txt)
	tests/capture.sh "${capture[@]}" "${refs[@]}"
fi

printf '\nTout est conforme.\n'
