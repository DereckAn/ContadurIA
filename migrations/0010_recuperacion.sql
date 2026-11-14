-- Fecha de alta de cada passkey (para listarlas en Seguridad).
ALTER TABLE passkeys ADD COLUMN creada TIMESTAMPTZ NOT NULL DEFAULT now();

-- Enlaces de recuperación por correo: se guarda sólo el sha256 del token; se borran al usarse.
CREATE TABLE recuperaciones (
    token_hash  TEXT PRIMARY KEY,
    usuario_id  UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    expira      TIMESTAMPTZ NOT NULL
);
