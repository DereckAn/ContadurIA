# ContadurIA

Contador virtual para personas en México. El plan completo está en [`plan.html`](plan.html).

**Estado:** Fase 0 — cuentas con passkeys, perfil fiscal y calendario de obligaciones.

## Correr en local

Requisitos: Rust 1.85+ y Docker.

```bash
# Postgres (una sola vez; después: docker start contaduria-db)
docker run -d --name contaduria-db \
  -e POSTGRES_USER=contaduria -e POSTGRES_PASSWORD=contaduria -e POSTGRES_DB=contaduria \
  -p 127.0.0.1:5432:5432 -v contaduria-db:/var/lib/postgresql/data postgres:17

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
templates/         páginas (Askama + Pico CSS)
static/passkey.js  WebAuthn en el navegador
migrations/        esquema de Postgres
```
