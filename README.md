# ContadurIA

Contador virtual para personas en México. El plan completo está en [`plan.html`](plan.html).

**Estado:** Fase 1 en curso — cuentas con passkeys, perfil fiscal, calendario y descarga de CFDI del SAT.

## Correr en local

Requisitos: Rust 1.85+, Docker y `pdftotext` para leer estados de cuenta (`brew install poppler`).
Opcional: [Ollama](https://ollama.com) para estados de bancos distintos de BBVA.

```bash
# Postgres (una sola vez; después: docker start contaduria-db)
docker run -d --name contaduria-db \
  -e POSTGRES_USER=contaduria -e POSTGRES_PASSWORD=contaduria -e POSTGRES_DB=contaduria \
  -p 127.0.0.1:5432:5432 -v contaduria-db:/var/lib/postgresql/data postgres:17

# e.firma en WebAssembly (una sola vez y cada vez que cambie efirma/)
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
./build-wasm.sh

cargo run
```

Abre <http://localhost:3000>. Las migraciones corren solas al arrancar.

| Variable | Por defecto | Para qué |
| --- | --- | --- |
| `DATABASE_URL` | `postgres://contaduria:contaduria@localhost/contaduria` | Conexión a Postgres |
| `APP_URL` | `http://localhost:3000` | Origen de las passkeys; debe coincidir con la URL del navegador |
| `BIND` | `127.0.0.1:3000` | Dirección donde escucha el servidor |

Las passkeys sólo funcionan en `localhost` o con HTTPS. Usa `localhost`, no `127.0.0.1`.

## Pruebas

```bash
cargo test
```

## Estructura

```text
src/main.rs        arranque, rutas, sesiones
src/auth.rs        crear cuenta y entrar con passkeys
src/perfil.rs      perfil fiscal e inicio
src/calendario.rs  obligaciones por régimen (con pruebas)
src/sat.rs         Descarga Masiva: reenvía al SAT mensajes ya firmados
src/cfdi.rs        lectura de CFDI y paquetes .zip
efirma/            e.firma en el navegador (Rust → WebAssembly)
templates/         páginas (Askama + Pico CSS)
static/            JavaScript del navegador
migrations/        esquema de Postgres
```

Los mensajes al SAT se prueban contra los de
[phpcfdi/sat-ws-descarga-masiva](https://github.com/phpcfdi/sat-ws-descarga-masiva) (MIT),
con su e.firma de pruebas `EKU9003173C9` en `efirma/pruebas/`.

## Estilos (Tailwind sin Node)

El CSS se compila con el binario `tailwindcss` (standalone, no requiere Node) a `static/estilo.css`.

```bash
# una sola vez: descarga el binario (ignorado por git)
curl -sL -o tailwindcss https://github.com/tailwindlabs/tailwindcss/releases/download/v3.4.17/tailwindcss-linux-x64
chmod +x tailwindcss

# cada vez que cambien templates/ o styles/input.css:
./build-css.sh
```

Tailwind descarta las clases que no aparecen en `templates/`, así que **si agregas clases nuevas en una plantilla, vuelve a correr `./build-css.sh`** o no se verán. `static/estilo.css` sí se versiona (se sirve tal cual); el binario `tailwindcss` no.
