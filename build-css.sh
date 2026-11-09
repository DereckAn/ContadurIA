#!/usr/bin/env bash
# Compila el CSS de Tailwind a static/estilo.css (sin Node). Corre tras cambiar templates/ o styles/.
set -e
cd "$(dirname "$0")"
./tailwindcss -c tailwind.config.js -i styles/input.css -o static/estilo.css --minify
echo "static/estilo.css generado"
