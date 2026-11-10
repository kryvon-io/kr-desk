#!/usr/bin/env bash
# Checagem de tipos (clippy) do código `cfg(windows)` no Linux, sem o target
# Windows instalado. Só para desenvolvimento: NÃO gera binários válidos.
#
# Como funciona: copia o repositório para um diretório temporário, troca
# `cfg(windows)` por `cfg(unix)` nos crates do projeto e aplica [patch] nos
# crates `windows-*` (cópias do registry com as travas de plataforma
# removidas). Assim o rustc verifica nomes, tipos e assinaturas das APIs
# Win32 usadas.
#
# Uso: scripts/check-windows-on-linux.sh -p kr-capture-win -p kr-codec-win --all-targets
set -euo pipefail

SRC="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${KR_WINCHECK_DIR:-/tmp/kr-wincheck}"
REPO="$WORK/repo"
PATCHED="$WORK/patched"
REGISTRY="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/ | head -1)"

cd "$SRC"
cargo fetch -q

# Versões dos crates windows-* usados (Cargo.lock).
mapfile -t CRATES < <(awk '/^name = "windows(-core|-future|-threading|-collections|-link|-numerics|-result|-strings)?"$/{n=$3; getline; print substr(n,2,length(n)-2) "-" substr($3,2,length($3)-2)}' Cargo.lock | sort -u)

rm -rf "$PATCHED" && mkdir -p "$PATCHED"
PATCH_SECTION=$'\n[patch.crates-io]\n'
for c in "${CRATES[@]}"; do
  cp -r "$REGISTRY/$c" "$PATCHED/"
  name="${c%-*}"
  PATCH_SECTION+="$name = { path = \"$PATCHED/$c\" }"$'\n'
done
# Remove as travas de plataforma.
sed -i 's/^#!\[cfg(windows)\]//' "$PATCHED"/windows-0.*/src/lib.rs
grep -rl "cfg(windows)\|cfg(not(windows))" "$PATCHED" | xargs -r sed -i 's/cfg(not(windows))/cfg(any())/g; s/cfg(windows)/cfg(all())/g'
# windows-link: usa a variante `extern` simples (sem raw-dylib).
LINK="$(ls -d "$PATCHED"/windows-link-*/src/lib.rs)"
python3 - "$LINK" <<'PY'
import sys, re
p = sys.argv[1]
s = open(p).read()
s = re.sub(r'#\[cfg\(all\(windows, target_arch = "x86"\)\)\]', '#[cfg(any())]', s)
s = re.sub(r'#\[cfg\(all\(windows, not\(target_arch = "x86"\)\)\)\]', '#[cfg(any())]', s)
s = re.sub(r'#\[cfg\(all\(all\(\), target_arch = "x86"\)\)\]', '#[cfg(any())]', s)
s = re.sub(r'#\[cfg\(all\(all\(\), not\(target_arch = "x86"\)\)\)\]', '#[cfg(any())]', s)
i = s.rfind('#[cfg(any())]')
s = s[:i] + '#[cfg(all())]' + s[i + len('#[cfg(any())]'):]
open(p, 'w').write(s)
PY

# Cópia do repositório com cfg(windows) → cfg(unix).
rm -rf "$REPO" && mkdir -p "$REPO"
tar -C "$SRC" --exclude=./target --exclude=./node_modules --exclude='*/node_modules' --exclude=./.git -cf - . | tar -C "$REPO" -xf -
cd "$REPO"
grep -rl --include='*.rs' --include='Cargo.toml' "cfg(windows)\|cfg(not(windows))" crates apps/desktop/src-tauri \
  | xargs -r sed -i 's/cfg(not(windows))/cfg(not(unix))/g; s/cfg(windows)/cfg(unix)/g'
# Sem `std` no crate windows (ele usaria std::os::windows).
sed -i 's/^windows = { version = "\([^"]*\)", features = \[/windows = { version = "\1", default-features = false, features = [/' Cargo.toml
printf '%s' "$PATCH_SECTION" >> Cargo.toml

CARGO_TARGET_DIR="$WORK/target" cargo clippy -q "$@" -- -D warnings 2>&1 \
  | awk '/^(warning|error)/{show = ($0 !~ /ambiguous import visibility/)} show' \
  | grep -v "$PATCHED" || true
echo "check-windows-on-linux: concluído (sem saída acima = sem problemas)"
